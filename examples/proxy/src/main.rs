use rust_raknet::{RaknetListener, RaknetSocket, Reliability};
use std::{
    error::Error,
    io::{self, ErrorKind},
    net::SocketAddr,
    num::NonZeroUsize,
    process,
};

fn usage(program: &str) -> ! {
    eprintln!("Usage: {program} -l <LOCAL_ADDR> -r <REMOTE_ADDR> [--socket-shards <COUNT>] [--batch-messages]");
    process::exit(2);
}

fn parse_socket_addr(value: String, option: &str) -> Result<SocketAddr, io::Error> {
    value.parse().map_err(|error| {
        io::Error::new(
            ErrorKind::InvalidInput,
            format!("invalid address for {option}: {error}"),
        )
    })
}

fn parse_args(
    mut args: impl Iterator<Item = String>,
) -> Result<(SocketAddr, SocketAddr, NonZeroUsize, bool), io::Error> {
    let mut local_address = None;
    let mut remote_address = None;
    let mut shards = None;
    let mut batch_messages = false;

    while let Some(option) = args.next() {
        if option == "--batch-messages" {
            if std::mem::replace(&mut batch_messages, true) {
                return Err(io::Error::new(
                    ErrorKind::InvalidInput,
                    "duplicate batch option",
                ));
            }
            continue;
        }
        let value = args.next().ok_or_else(|| {
            io::Error::new(
                ErrorKind::InvalidInput,
                format!("missing value for {option}"),
            )
        })?;
        if option == "--socket-shards" {
            let count = value.parse::<NonZeroUsize>().map_err(|error| {
                io::Error::new(
                    ErrorKind::InvalidInput,
                    format!("invalid shard count: {error}"),
                )
            })?;
            if shards.replace(count).is_some() {
                return Err(io::Error::new(
                    ErrorKind::InvalidInput,
                    "duplicate shard count",
                ));
            }
            continue;
        }
        let address = parse_socket_addr(value, &option)?;
        let destination = match option.as_str() {
            "-l" | "--local_address" | "--local-address" => &mut local_address,
            "-r" | "--remote_address" | "--remote-address" => &mut remote_address,
            _ => {
                return Err(io::Error::new(
                    ErrorKind::InvalidInput,
                    format!("unknown option: {option}"),
                ));
            }
        };

        if destination.replace(address).is_some() {
            return Err(io::Error::new(
                ErrorKind::InvalidInput,
                format!("{option} may only be specified once"),
            ));
        }
    }

    let local_address = local_address
        .ok_or_else(|| io::Error::new(ErrorKind::InvalidInput, "the local address is required"))?;
    let remote_address = remote_address
        .ok_or_else(|| io::Error::new(ErrorKind::InvalidInput, "the remote address is required"))?;
    Ok((
        local_address,
        remote_address,
        shards.unwrap_or(NonZeroUsize::MIN),
        batch_messages,
    ))
}

async fn relay(client: RaknetSocket, upstream: RaknetSocket, batch_messages: bool) {
    println!("RakNet proxy connection established");

    async fn forward<const BATCH: bool>(
        source: &RaknetSocket,
        destination: &RaknetSocket,
    ) -> rust_raknet::error::Result<()> {
        let mut batch = 0;
        let mut packets = Vec::with_capacity(if BATCH { 8 } else { 0 });
        loop {
            if BATCH {
                source.recv_bytes_batch(&mut packets, 8).await?;
                destination
                    .send_bytes_batch(&packets, Reliability::ReliableOrdered)
                    .await?;
                batch += packets.len();
            } else {
                let packet = source.recv_bytes().await?;
                destination
                    .send_bytes(packet, Reliability::ReliableOrdered)
                    .await?;
                batch += 1;
            }
            if batch >= 8 {
                // Bound each ready batch so other connections and UDP receivers
                // can run before a burst fills a shared socket's receive buffer.
                batch = 0;
                tokio::task::yield_now().await;
            }
        }
    }

    // A blocked send in one direction must not stall the reverse receive path.
    let result = if batch_messages {
        tokio::try_join!(
            forward::<true>(&client, &upstream),
            forward::<true>(&upstream, &client)
        )
    } else {
        tokio::try_join!(
            forward::<false>(&client, &upstream),
            forward::<false>(&upstream, &client)
        )
    };
    if let Err(error) = result {
        eprintln!("RakNet relay stopped: {error}");
    }

    let _ = client.close().await;
    let _ = upstream.close().await;
    println!("RakNet proxy connection closed");
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let mut args = std::env::args();
    let program = args.next().unwrap_or_else(|| "raknet-proxy".to_owned());
    let (local_address, remote_address, shards, batch_messages) =
        parse_args(args).unwrap_or_else(|error| {
            eprintln!("{error}");
            usage(&program)
        });

    #[cfg(target_os = "linux")]
    let mut listener = RaknetListener::bind_with_socket_shards(&local_address, shards).await?;
    #[cfg(not(target_os = "linux"))]
    let mut listener = {
        if shards.get() != 1 {
            return Err(
                io::Error::new(ErrorKind::Unsupported, "socket sharding requires Linux").into(),
            );
        }
        RaknetListener::bind(&local_address).await?
    };
    listener.listen().await;
    loop {
        let client = listener.accept().await?;
        tokio::spawn(async move {
            // Bedrock 26.52.3 negotiates RakNet protocol 11 in legacy RakNet mode.
            // Use the client's negotiated version for the upstream connection.
            let version = match client.raknet_version() {
                Ok(version) => version,
                Err(error) => {
                    eprintln!("could not read the client's RakNet version: {error}");
                    let _ = client.close().await;
                    return;
                }
            };

            let upstream = match tokio::time::timeout(
                std::time::Duration::from_secs(10),
                RaknetSocket::connect_with_version(&remote_address, version),
            )
            .await
            {
                Ok(Ok(socket)) => socket,
                Ok(Err(error)) => {
                    eprintln!("could not connect to the remote RakNet server: {error}");
                    let _ = client.close().await;
                    return;
                }
                Err(_) => {
                    eprintln!("remote RakNet handshake timed out");
                    let _ = client.close().await;
                    return;
                }
            };
            relay(client, upstream, batch_messages).await;
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn shard_option_defaults_to_one_and_rejects_invalid_or_duplicate_counts() {
        fn args(extra: &[&str]) -> impl Iterator<Item = String> {
            ["-l", "127.0.0.1:19144", "-r", "127.0.0.1:19142"]
                .into_iter()
                .chain(extra.iter().copied())
                .map(str::to_owned)
                .collect::<Vec<_>>()
                .into_iter()
        }
        assert_eq!(parse_args(args(&[])).unwrap().2.get(), 1);
        assert!(!parse_args(args(&[])).unwrap().3);
        assert!(parse_args(args(&["--batch-messages"])).unwrap().3);
        assert!(parse_args(args(&["--batch-messages", "--batch-messages"])).is_err());
        assert_eq!(
            parse_args(args(&["--socket-shards", "4"])).unwrap().2.get(),
            4
        );
        assert!(parse_args(args(&["--socket-shards", "0"])).is_err());
        assert!(parse_args(args(&["--socket-shards", "4", "--socket-shards", "2"])).is_err());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn relay_keeps_both_directions_moving_under_backpressure() {
        for batch_messages in [false, true] {
            tokio::time::timeout(Duration::from_secs(20), async {
                let mut frontend = RaknetListener::bind(&"127.0.0.1:0".parse().unwrap())
                    .await
                    .unwrap();
                let front_addr = frontend.local_addr().unwrap();
                frontend.listen().await;
                let client = RaknetSocket::connect_with_version(&front_addr, 11)
                    .await
                    .unwrap();
                let proxy_client = frontend.accept().await.unwrap();
                let mut backend = RaknetListener::bind(&"127.0.0.1:0".parse().unwrap())
                    .await
                    .unwrap();
                let back_addr = backend.local_addr().unwrap();
                backend.listen().await;
                let proxy_upstream = RaknetSocket::connect_with_version(&back_addr, 11)
                    .await
                    .unwrap();
                let server = backend.accept().await.unwrap();
                let task = tokio::spawn(relay(proxy_client, proxy_upstream, batch_messages));
                async fn exchange(socket: &RaknetSocket, outgoing: u8, incoming: u8) {
                    let send = async {
                        for index in 0..2000_u32 {
                            let mut payload = vec![0xfe; 800];
                            payload[1] = outgoing;
                            payload[2..6].copy_from_slice(&index.to_le_bytes());
                            socket
                                .send(&payload, Reliability::ReliableOrdered)
                                .await
                                .unwrap();
                        }
                        socket.flush().await.unwrap();
                    };
                    let receive = async {
                        tokio::time::sleep(Duration::from_millis(100)).await;
                        for index in 0..2000_u32 {
                            let payload = socket.recv().await.unwrap();
                            assert_eq!(payload.len(), 800);
                            assert_eq!(payload[1], incoming);
                            assert_eq!(&payload[2..6], &index.to_le_bytes());
                        }
                    };
                    tokio::join!(send, receive);
                }
                tokio::join!(exchange(&client, 1, 2), exchange(&server, 2, 1));
                client.close().await.unwrap();
                task.await.unwrap();
                server.close().await.unwrap();
                frontend.close().await.unwrap();
                backend.close().await.unwrap();
            })
            .await
            .expect("bidirectional proxy forwarding stalled");
        }
    }
}
