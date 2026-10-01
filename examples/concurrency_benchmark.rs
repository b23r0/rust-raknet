//! Concurrent ordered echo benchmark. Run in an isolated network namespace.
use rust_raknet::{RaknetSocket, Reliability};
use std::{
    error::Error,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::{
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
async fn main() -> Result<(), Box<dyn Error>> {
    let args: Vec<_> = std::env::args().collect();
    if args.len() != 5 {
        return Err("usage: concurrency_benchmark ADDRESS CONNECTIONS MESSAGES PAYLOAD".into());
    }
    let address = args[1].parse()?;
    let count: usize = args[2].parse()?;
    let messages: usize = args[3].parse()?;
    let size: usize = args[4].parse()?;
    if count == 0 || messages == 0 || size < 17 {
        return Err("invalid counts".into());
    }
    tokio::time::timeout(
        Duration::from_secs(180),
        run(address, count, messages, size),
    )
    .await??;
    Ok(())
}

async fn run(
    address: std::net::SocketAddr,
    count: usize,
    messages: usize,
    size: usize,
) -> Result<(), Box<dyn Error>> {
    let setup = Instant::now();
    let dialing = Arc::new(Semaphore::new(8));
    let mut tasks = JoinSet::new();
    for id in 0..count {
        let dialing = dialing.clone();
        tasks.spawn(async move {
            let _permit = dialing.acquire().await.unwrap();
            let socket = RaknetSocket::connect_with_version(&address, 11).await?;
            Ok::<_, rust_raknet::error::RaknetError>((id, socket))
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
    for (id, socket) in sockets {
        let barrier = barrier.clone();
        let start_gate = start_gate.clone();
        exchanges.spawn(async move {
            let mut rtts = Vec::with_capacity(20);
            let mut expected = payload(id, 0, size);
            let mut outgoing = expected.clone();
            for message in 0..20 {
                expected[9..17].copy_from_slice(&(message as u64).to_le_bytes());
                let start = Instant::now();
                socket.send(&expected, Reliability::ReliableOrdered).await?;
                let actual = socket.recv().await?;
                if actual != expected {
                    return Err(rust_raknet::error::RaknetError::PacketParseError);
                }
                rtts.push(start.elapsed().as_nanos());
            }
            barrier.wait().await;
            start_gate.wait().await;
            // Match the C reference's sliding application window. Refill a slot
            // as each echo arrives instead of waiting for a whole batch.
            let mut sent = 0;
            while sent < messages.min(16) {
                outgoing[9..17].copy_from_slice(&((sent + 20) as u64).to_le_bytes());
                socket.send(&outgoing, Reliability::ReliableOrdered).await?;
                sent += 1;
            }
            for received in 0..messages {
                expected[9..17].copy_from_slice(&((received + 20) as u64).to_le_bytes());
                let actual = socket.recv().await?;
                if actual != expected {
                    return Err(rust_raknet::error::RaknetError::PacketParseError);
                }
                if sent < messages {
                    outgoing[9..17].copy_from_slice(&((sent + 20) as u64).to_le_bytes());
                    socket.send(&outgoing, Reliability::ReliableOrdered).await?;
                    sent += 1;
                }
            }
            // Keep every connection open until all peers finish the measured burst.
            Ok::<_, rust_raknet::error::RaknetError>((socket, rtts))
        });
    }
    barrier.wait().await;
    let started = Instant::now();
    start_gate.wait().await;
    let mut retained = Vec::with_capacity(count);
    let mut rtts = Vec::with_capacity(count * 20);
    while let Some(result) = exchanges.join_next().await {
        let (socket, values) = result??;
        retained.push(socket);
        rtts.extend(values);
    }
    let elapsed = started.elapsed().as_secs_f64();
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
    println!("Verified ordered echoes: {}", count * (messages + 20));
    drop(retained);
    Ok::<_, Box<dyn Error>>(())
}
