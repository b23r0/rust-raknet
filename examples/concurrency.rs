//! Concurrent RakNet echo validation; run only inside an isolated network.
use rust_raknet::{RaknetSocket, Reliability};
use std::{
    error::Error,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::{sync::Semaphore, task::JoinSet};

#[tokio::main(worker_threads = 4)]
async fn main() -> Result<(), Box<dyn Error>> {
    let args: Vec<_> = std::env::args().collect();
    let address = args
        .get(1)
        .ok_or("usage: concurrency ADDRESS [CONNECTIONS] [MESSAGES]")?
        .parse()?;
    let connections: usize = args.get(2).map_or(Ok(512), |n| n.parse())?;
    let messages: usize = args.get(3).map_or(Ok(100), |n| n.parse())?;
    if connections == 0 || messages == 0 {
        return Err("counts must be positive".into());
    }
    tokio::time::timeout(Duration::from_secs(120), async {
        let started = Instant::now();
        let dialing = Arc::new(Semaphore::new(8));
        let mut tasks = JoinSet::new();
        for index in 0..connections {
            let dialing = dialing.clone();
            tasks.spawn(async move {
                let _permit = dialing.acquire().await.unwrap();
                let socket = RaknetSocket::connect_with_version(&address, 11).await?;
                Ok::<_, rust_raknet::error::RaknetError>((index, socket))
            });
        }
        let mut sockets = Vec::with_capacity(connections);
        while let Some(result) = tasks.join_next().await { sockets.push(result??); }
        let handshake_seconds = started.elapsed().as_secs_f64();
        let transfer = Instant::now();
        let mut exchanges = JoinSet::new();
        for (index, socket) in sockets {
            exchanges.spawn(async move {
                fn payload(connection: usize, message: usize) -> Vec<u8> {
                    let size = [64, 800, 4096][message % 3];
                    let mut bytes = vec![0xfe; size];
                    bytes[1..9].copy_from_slice(&(connection as u64).to_le_bytes());
                    bytes[9..17].copy_from_slice(&(message as u64).to_le_bytes());
                    bytes
                }
                let sender = async {
                    for message in 0..messages {
                        socket.send(&payload(index, message), Reliability::ReliableOrdered).await?;
                    }
                    socket.flush().await
                };
                let receiver = async {
                    // Exercise a slow application without blocking protocol ACKs.
                    tokio::time::sleep(Duration::from_millis(30)).await;
                    for message in 0..messages {
                        let actual = socket.recv().await?;
                        if actual != payload(index, message) {
                            return Err(rust_raknet::error::RaknetError::PacketParseError);
                        }
                    }
                    Ok(())
                };
                tokio::try_join!(sender, receiver)?;
                socket.close().await
            });
        }
        while let Some(result) = exchanges.join_next().await { result??; }
        println!("PASS: {connections} simultaneous connections, {} unique ordered echoes; handshake={handshake_seconds:.3}s, transfer={:.3}s", connections * messages, transfer.elapsed().as_secs_f64());
        Ok::<_, Box<dyn Error>>(())
    }).await?
}
