//! Concurrent ordered echo benchmark. Run in an isolated network namespace.
use rust_raknet::{RaknetSocket, Reliability};
use std::{
    error::Error,
    sync::Arc,
    time::{Duration, Instant},
};
type BenchError = Box<dyn Error + Send + Sync>;

use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::{Barrier, Semaphore},
    task::JoinSet,
};

fn payload(connection: usize, message: usize, size: usize) -> Vec<u8> {
    let mut data = vec![0xfe; size];
    data[1..9].copy_from_slice(&(connection as u64).to_le_bytes());
    data[9..17].copy_from_slice(&(message as u64).to_le_bytes());
    data
}
fn percentile(values: &[u128], percentile: usize) -> f64 {
    values[(values.len() * percentile).div_ceil(100) - 1] as f64 / 1000.0
}
#[tokio::main(worker_threads = 4)]
async fn main() -> Result<(), BenchError> {
    let mut args: Vec<_> = std::env::args().collect();
    if args.get(1).is_some_and(|arg| arg == "--tcp-server") {
        if args.len() != 3 {
            return Err("usage: concurrency_benchmark --tcp-server ADDRESS".into());
        }
        return tcp_server(args[2].parse()?).await;
    }
    let loaded_rtt = args.last().is_some_and(|arg| arg == "--loaded-rtt");
    if loaded_rtt {
        args.pop();
    }
    let tcp = args.get(1).is_some_and(|arg| arg == "--tcp");
    if tcp {
        args.remove(1);
    }
    let batch = args.get(1).is_some_and(|arg| arg == "--batch");
    if batch {
        args.remove(1);
    }
    if batch && tcp {
        return Err("--batch is only supported for RakNet".into());
    }
    if args.len() != 5 {
        return Err(
            "usage: concurrency_benchmark [--tcp | --batch] ADDRESS CONNECTIONS MESSAGES PAYLOAD [--loaded-rtt]"
                .into(),
        );
    }
    let address = args[1].parse()?;
    let count: usize = args[2].parse()?;
    let messages: usize = args[3].parse()?;
    let size: usize = args[4].parse()?;
    if count == 0 || messages == 0 || size < 17 {
        return Err("invalid counts".into());
    }
    if loaded_rtt {
        tokio::time::timeout(
            Duration::from_secs(180),
            run::<true>(address, count, messages, size, tcp, batch),
        )
        .await??;
    } else {
        tokio::time::timeout(
            Duration::from_secs(180),
            run::<false>(address, count, messages, size, tcp, batch),
        )
        .await??;
    }
    Ok(())
}

async fn run<const LOADED: bool>(
    address: std::net::SocketAddr,
    count: usize,
    messages: usize,
    size: usize,
    tcp: bool,
    batch: bool,
) -> Result<(), BenchError> {
    println!(
        "Sending API: {}",
        if tcp {
            "TCP complete-record writes"
        } else if batch {
            "send_bytes_batch (ready messages only)"
        } else {
            "send (no batch API)"
        }
    );
    let setup = Instant::now();
    let dialing = Arc::new(Semaphore::new(8));
    let mut tasks = JoinSet::new();
    for id in 0..count {
        let dialing = dialing.clone();
        tasks.spawn(async move {
            let _permit = dialing.acquire().await.unwrap();
            let socket = EchoConnection::connect(address, tcp).await?;
            Ok::<_, BenchError>((id, socket))
        });
    }
    let mut sockets = Vec::with_capacity(count);
    while let Some(result) = tasks.join_next().await {
        sockets.push(result??);
    }
    let setup_seconds = setup.elapsed().as_secs_f64();
    let barrier = Arc::new(Barrier::new(count + 1));
    let start_gate = Arc::new(Barrier::new(count + 1));
    let mut exchanges = JoinSet::new();
    for (id, mut socket) in sockets {
        let barrier = barrier.clone();
        let start_gate = start_gate.clone();
        exchanges.spawn(async move {
            let mut rtts = Vec::with_capacity(if LOADED { messages.max(20) } else { 20 });
            let mut stamps = if LOADED {
                vec![Instant::now(); 16]
            } else {
                Vec::new()
            };
            let mut expected = payload(id, 0, size);
            let mut outgoing = expected.clone();
            let mut actual = Vec::with_capacity(size);
            for message in 0..20 {
                expected[9..17].copy_from_slice(&(message as u64).to_le_bytes());
                let start = Instant::now();
                socket.send(&expected).await?;
                socket.recv(size, &mut actual).await?;
                if actual != expected {
                    return Err::<_, BenchError>("echo payload or order mismatch".into());
                }
                rtts.push(start.elapsed().as_nanos());
            }
            if LOADED {
                rtts.clear();
            }
            barrier.wait().await;
            start_gate.wait().await;
            // Match the C reference's sliding application window. Refill a slot
            // as each echo arrives instead of waiting for a whole batch.
            if batch {
                let EchoConnection::RakNet(ref connection) = socket else {
                    unreachable!()
                };
                let mut sent = 0;
                let mut received = 0;
                let mut incoming = Vec::with_capacity(16);
                let mut pending = Vec::with_capacity(16);
                for message in 0..messages.min(16) {
                    pending.push(rust_raknet::Bytes::from(payload(id, message + 20, size)));
                    if LOADED {
                        stamps[message % 16] = Instant::now();
                    }
                    sent += 1;
                }
                connection
                    .send_bytes_batch(&pending, Reliability::ReliableOrdered)
                    .await?;
                while received < messages {
                    connection.recv_bytes_batch(&mut incoming, 16).await?;
                    for actual in &incoming {
                        expected[9..17].copy_from_slice(&((received + 20) as u64).to_le_bytes());
                        if actual.as_ref() != expected {
                            return Err::<_, BenchError>("echo payload or order mismatch".into());
                        }
                        if LOADED {
                            rtts.push(stamps[received % 16].elapsed().as_nanos());
                        }
                        received += 1;
                    }
                    pending.clear();
                    for _ in 0..incoming.len().min(messages - sent) {
                        pending.push(rust_raknet::Bytes::from(payload(id, sent + 20, size)));
                        if LOADED {
                            stamps[sent % 16] = Instant::now();
                        }
                        sent += 1;
                    }
                    if !pending.is_empty() {
                        connection
                            .send_bytes_batch(&pending, Reliability::ReliableOrdered)
                            .await?;
                    }
                }
            } else {
                let mut sent = 0;
                while sent < messages.min(16) {
                    outgoing[9..17].copy_from_slice(&((sent + 20) as u64).to_le_bytes());
                    if LOADED {
                        stamps[sent % 16] = Instant::now();
                    }
                    socket.send(&outgoing).await?;
                    sent += 1;
                }
                for received in 0..messages {
                    expected[9..17].copy_from_slice(&((received + 20) as u64).to_le_bytes());
                    socket.recv(size, &mut actual).await?;
                    if actual != expected {
                        return Err::<_, BenchError>("echo payload or order mismatch".into());
                    }
                    if LOADED {
                        rtts.push(stamps[received % 16].elapsed().as_nanos());
                    }
                    if sent < messages {
                        outgoing[9..17].copy_from_slice(&((sent + 20) as u64).to_le_bytes());
                        if LOADED {
                            stamps[sent % 16] = Instant::now();
                        }
                        socket.send(&outgoing).await?;
                        sent += 1;
                    }
                }
            }
            // Keep every connection open until all peers finish the measured burst.
            Ok::<_, BenchError>((socket, rtts))
        });
    }
    barrier.wait().await;
    let started = Instant::now();
    start_gate.wait().await;
    let mut retained = Vec::with_capacity(count);
    let mut rtts = Vec::with_capacity(count * if LOADED { messages } else { 20 });
    while let Some(result) = exchanges.join_next().await {
        let (socket, values) = result??;
        retained.push(socket);
        rtts.extend(values);
    }
    let elapsed = started.elapsed().as_secs_f64();
    println!("RTT samples: {}", rtts.len());
    rtts.sort_unstable();
    println!(
        "Connections: {count}\nMessages per connection: {messages}\nPayload size: {size} bytes\nSetup: {setup_seconds:.6} s\nElapsed: {elapsed:.6} s"
    );
    println!(
        "Echo payload throughput (per direction): {:.2} MiB/s",
        count as f64 * messages as f64 * size as f64 / 1048576.0 / elapsed
    );
    println!(
        "RTT p50: {:.1} us\nRTT p95: {:.1} us\nRTT p99: {:.1} us",
        percentile(&rtts, 50),
        percentile(&rtts, 95),
        percentile(&rtts, 99)
    );
    println!(
        "RTT measurement: {}",
        if LOADED {
            "per-message throughout measured burst"
        } else {
            "sequential warmup before burst"
        }
    );
    println!("Verified ordered echoes: {}", count * (messages + 20));
    drop(retained);
    Ok(())
}

// TCP records use a four-byte length prefix and one complete-record write.
// Both buffers are retained across messages, just like the payload templates.
enum EchoConnection {
    RakNet(RaknetSocket),
    Tcp { stream: TcpStream, record: Vec<u8> },
}

impl EchoConnection {
    async fn connect(address: std::net::SocketAddr, tcp: bool) -> Result<Self, BenchError> {
        if tcp {
            let stream = TcpStream::connect(address).await?;
            stream.set_nodelay(true)?;
            Ok(Self::Tcp {
                stream,
                record: Vec::new(),
            })
        } else {
            Ok(Self::RakNet(
                RaknetSocket::connect_with_version(&address, 11).await?,
            ))
        }
    }

    async fn send(&mut self, payload: &[u8]) -> Result<(), BenchError> {
        match self {
            Self::RakNet(socket) => socket.send(payload, Reliability::ReliableOrdered).await?,
            Self::Tcp { stream, record, .. } => {
                let length = u32::try_from(payload.len())?;
                record.clear();
                record.extend_from_slice(&length.to_le_bytes());
                record.extend_from_slice(payload);
                stream.write_all(record).await?;
            }
        }
        Ok(())
    }

    async fn recv(&mut self, size: usize, incoming: &mut Vec<u8>) -> Result<(), BenchError> {
        match self {
            Self::RakNet(socket) => *incoming = socket.recv().await?,
            Self::Tcp { stream, .. } => {
                let length = stream.read_u32_le().await? as usize;
                if length != size {
                    return Err("unexpected TCP record length".into());
                }
                incoming.resize(length, 0);
                stream.read_exact(incoming).await?;
            }
        }
        Ok(())
    }
}

async fn echo_tcp(mut stream: TcpStream) -> Result<(), BenchError> {
    stream.set_nodelay(true)?;
    let mut record = Vec::new();
    loop {
        let mut header = [0; 4];
        match stream.read_exact(&mut header).await {
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(()),
            Err(error) => return Err(error.into()),
        }
        let length = u32::from_le_bytes(header) as usize;
        if length > 65536 {
            return Err("TCP record exceeds benchmark limit".into());
        }
        record.resize(length + 4, 0);
        record[..4].copy_from_slice(&header);
        stream.read_exact(&mut record[4..]).await?;
        stream.write_all(&record).await?;
    }
}

#[cfg(target_os = "linux")]
async fn tcp_server(address: std::net::SocketAddr) -> Result<(), BenchError> {
    use socket2::{Domain, Protocol, Socket, Type};
    let mut listeners = Vec::with_capacity(4);
    for _ in 0..4 {
        let socket = Socket::new(
            Domain::for_address(address),
            Type::STREAM,
            Some(Protocol::TCP),
        )?;
        socket.set_reuse_address(true)?;
        socket.set_reuse_port(true)?;
        socket.set_nonblocking(true)?;
        socket.bind(&address.into())?;
        socket.listen(4096)?;
        listeners.push(TcpListener::from_std(socket.into())?);
    }
    println!("TCP echo listening on {address} with four receive sockets");
    let mut tasks = JoinSet::new();
    for listener in listeners {
        tasks.spawn(async move {
            loop {
                let (stream, _) = listener.accept().await?;
                tokio::spawn(async move {
                    if let Err(error) = echo_tcp(stream).await {
                        eprintln!("TCP echo failed: {error}");
                    }
                });
            }
            #[allow(unreachable_code)]
            Ok::<(), BenchError>(())
        });
    }
    while let Some(result) = tasks.join_next().await {
        result??;
    }
    Ok(())
}

#[cfg(not(target_os = "linux"))]
async fn tcp_server(_address: std::net::SocketAddr) -> Result<(), BenchError> {
    Err("The four-socket TCP benchmark server requires Linux".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn tcp_records_preserve_payload_order_and_reuse_buffers() -> Result<(), BenchError> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?;
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await?;
            echo_tcp(stream).await
        });
        let mut connection = EchoConnection::connect(address, true).await?;
        let mut received = Vec::new();
        for size in [17, 64, 800, 4096, 64] {
            for message in 0..32 {
                let expected = payload(3, message, size);
                connection.send(&expected).await?;
                connection.recv(size, &mut received).await?;
                assert_eq!(received, expected);
            }
        }
        let capacity = received.capacity();
        connection.send(&payload(3, 40, 64)).await?;
        connection.recv(64, &mut received).await?;
        assert_eq!(received.capacity(), capacity);
        drop(connection);
        server.await??;
        Ok(())
    }
}
