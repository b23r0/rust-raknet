//! Synchronous counterpart of concurrency_benchmark; use a private network namespace.
#[cfg(not(feature = "blocking"))]
fn main() {
    eprintln!("Enable blocking: cargo run --features blocking --example blocking_benchmark");
    std::process::exit(2);
}

#[cfg(feature = "blocking")]
mod benchmark {
    use rust_raknet::{Reliability, blocking::RaknetRuntime};
    use std::{
        error::Error,
        net::SocketAddr,
        num::NonZeroUsize,
        sync::{Arc, Barrier},
        time::{Duration, Instant},
    };
    type BenchError = Box<dyn Error + Send + Sync>;
    fn payload(connection: usize, message: usize, size: usize) -> Vec<u8> {
        let mut data = vec![0xfe; size];
        data[1..9].copy_from_slice(&(connection as u64).to_le_bytes());
        data[9..17].copy_from_slice(&(message as u64).to_le_bytes());
        data
    }
    fn percentile(values: &[u128], percentile: usize) -> f64 {
        values[(values.len() * percentile).div_ceil(100) - 1] as f64 / 1000.0
    }
    pub fn main() -> Result<(), BenchError> {
        let mut args: Vec<_> = std::env::args().collect();
        let loaded = args.last().is_some_and(|arg| arg == "--loaded-rtt");
        if loaded {
            args.pop();
        }
        let batch = args.get(1).is_some_and(|arg| arg == "--batch");
        if batch {
            args.remove(1);
        }
        if args.len() != 5 {
            return Err("usage: blocking_benchmark [--batch] ADDRESS CONNECTIONS MESSAGES PAYLOAD [--loaded-rtt]".into());
        }
        let count: usize = args[2].parse()?;
        let messages: usize = args[3].parse()?;
        let size: usize = args[4].parse()?;
        if count == 0 || count > 256 || messages == 0 || size < 17 {
            return Err(
                "expected 1..=256 connections, positive messages, payload >= 17 bytes".into(),
            );
        }
        if loaded {
            run::<true>(args[1].parse()?, count, messages, size, batch)
        } else {
            run::<false>(args[1].parse()?, count, messages, size, batch)
        }
    }
    fn run<const LOADED: bool>(
        address: SocketAddr,
        count: usize,
        messages: usize,
        size: usize,
        batch: bool,
    ) -> Result<(), BenchError> {
        let runtime = RaknetRuntime::with_worker_threads(NonZeroUsize::new(4).unwrap())?;
        let setup = Instant::now();
        let mut sockets = Vec::with_capacity(count);
        for id in 0..count {
            sockets.push((
                id,
                runtime.connect_timeout(&address, Duration::from_secs(10))?,
            ));
        }
        let setup_seconds = setup.elapsed().as_secs_f64();
        let barrier = Arc::new(Barrier::new(count + 1));
        let start_gate = Arc::new(Barrier::new(count + 1));
        let mut exchanges = Vec::with_capacity(count);
        for (id, socket) in sockets {
            let barrier = barrier.clone();
            let start_gate = start_gate.clone();
            exchanges.push(std::thread::spawn(move || {
                let mut rtts = Vec::with_capacity(if LOADED { messages.max(20) } else { 20 });
                let mut stamps = if LOADED {
                    vec![Instant::now(); 16]
                } else {
                    Vec::new()
                };
                let mut expected = payload(id, 0, size);
                let mut outgoing = expected.clone();
                for message in 0..20 {
                    expected[9..17].copy_from_slice(&(message as u64).to_le_bytes());
                    let start = Instant::now();
                    socket.send(&expected, Reliability::ReliableOrdered)?;
                    let actual = socket.recv()?;
                    if actual != expected {
                        return Err::<_, BenchError>("echo payload or order mismatch".into());
                    }
                    rtts.push(start.elapsed().as_nanos());
                }
                if LOADED {
                    rtts.clear();
                }
                barrier.wait();
                start_gate.wait();
                // Match the C reference's sliding application window. Refill a slot
                // as each echo arrives instead of waiting for a whole batch.
                if batch {
                    let connection = &socket;
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
                    connection.send_bytes_batch(&pending, Reliability::ReliableOrdered)?;
                    while received < messages {
                        connection.recv_bytes_batch(&mut incoming, 16)?;
                        for actual in &incoming {
                            expected[9..17]
                                .copy_from_slice(&((received + 20) as u64).to_le_bytes());
                            if actual.as_ref() != expected {
                                return Err::<_, BenchError>(
                                    "echo payload or order mismatch".into(),
                                );
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
                            connection.send_bytes_batch(&pending, Reliability::ReliableOrdered)?;
                        }
                    }
                } else {
                    let mut sent = 0;
                    while sent < messages.min(16) {
                        outgoing[9..17].copy_from_slice(&((sent + 20) as u64).to_le_bytes());
                        if LOADED {
                            stamps[sent % 16] = Instant::now();
                        }
                        socket.send(&outgoing, Reliability::ReliableOrdered)?;
                        sent += 1;
                    }
                    for received in 0..messages {
                        expected[9..17].copy_from_slice(&((received + 20) as u64).to_le_bytes());
                        let actual = socket.recv()?;
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
                            socket.send(&outgoing, Reliability::ReliableOrdered)?;
                            sent += 1;
                        }
                    }
                }
                Ok::<_, BenchError>((socket, rtts))
            }));
        }
        barrier.wait();
        let started = Instant::now();
        start_gate.wait();
        let mut retained = Vec::with_capacity(count);
        let mut rtts = Vec::new();
        for task in exchanges {
            let (socket, values) = task.join().map_err(|_| "benchmark worker panicked")??;
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
            "RTT samples: {}\nRTT p50: {:.1} us\nRTT p95: {:.1} us\nRTT p99: {:.1} us",
            rtts.len(),
            percentile(&rtts, 50),
            percentile(&rtts, 95),
            percentile(&rtts, 99)
        );
        println!("Verified ordered echoes: {}", count * (messages + 20));
        drop(retained);
        Ok(())
    }
}
#[cfg(feature = "blocking")]
fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    benchmark::main()
}
