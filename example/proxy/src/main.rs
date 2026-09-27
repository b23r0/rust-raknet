use rust_raknet::{RaknetListener, RaknetSocket, Reliability};
use std::{
    error::Error,
    io::{self, ErrorKind},
    net::SocketAddr,
    process,
};

fn usage(program: &str) -> ! {
    eprintln!("Usage: {program} -l <LOCAL_ADDR> -r <REMOTE_ADDR>");
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
) -> Result<(SocketAddr, SocketAddr), io::Error> {
    let mut local_address = None;
    let mut remote_address = None;

    while let Some(option) = args.next() {
        let value = args.next().ok_or_else(|| {
            io::Error::new(
                ErrorKind::InvalidInput,
                format!("missing value for {option}"),
            )
        })?;
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
    Ok((local_address, remote_address))
}

async fn relay(client: RaknetSocket, upstream: RaknetSocket) {
    println!("RakNet proxy connection established");

    loop {
        tokio::select! {
            result = client.recv() => {
                let Ok(packet) = result else {
                    break;
                };
                if let Err(error) = upstream.send(&packet, Reliability::ReliableOrdered).await {
                    eprintln!("client-to-upstream relay failed: {error}");
                    break;
                }
            }
            result = upstream.recv() => {
                let Ok(packet) = result else {
                    break;
                };
                if let Err(error) = client.send(&packet, Reliability::ReliableOrdered).await {
                    eprintln!("upstream-to-client relay failed: {error}");
                    break;
                }
            }
        }
    }

    let _ = client.close().await;
    let _ = upstream.close().await;
    println!("RakNet proxy connection closed");
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let mut args = std::env::args();
    let program = args.next().unwrap_or_else(|| "raknet-proxy".to_owned());
    let (local_address, remote_address) = parse_args(args).unwrap_or_else(|error| {
        eprintln!("{error}");
        usage(&program)
    });

    let mut listener = RaknetListener::bind(&local_address).await?;
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

            let upstream = match RaknetSocket::connect_with_version(&remote_address, version).await
            {
                Ok(socket) => socket,
                Err(error) => {
                    eprintln!("could not connect to the remote RakNet server: {error}");
                    let _ = client.close().await;
                    return;
                }
            };
            relay(client, upstream).await;
        });
    }
}
