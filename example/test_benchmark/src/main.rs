use std::{
    error::Error,
    io::{self, ErrorKind},
    net::SocketAddr,
    time::{Duration, Instant},
};

use rust_raknet::{RaknetListener, RaknetSocket, Reliability};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
};

const DEFAULT_PACKET_COUNT: usize = 10_000;
const DEFAULT_PAYLOAD_SIZE: usize = 800;
const DEFAULT_WARMUP_COUNT: usize = 100;
const DEFAULT_LATENCY_SAMPLES: usize = 100;
const MAX_PAYLOAD_SIZE: usize = 64 * 1024 * 1024;
const TCP_RECORD_HEADER_SIZE: usize = std::mem::size_of::<u32>();

#[derive(Clone, Copy)]
enum Protocol {
    Tcp,
    RakNet,
}

impl Protocol {
    fn name(self) -> &'static str {
        match self {
            Self::Tcp => "TCP",
            Self::RakNet => "RakNet",
        }
    }
}

#[derive(Clone, Copy)]
enum Mode {
    Client,
    Server,
}

struct Config {
    protocol: Protocol,
    mode: Mode,
    address: String,
    packet_count: usize,
    payload_size: usize,
    warmup_count: usize,
    latency_samples: usize,
    raknet_mtu: u16,
}

fn invalid_input(message: impl Into<String>) -> io::Error {
    io::Error::new(ErrorKind::InvalidInput, message.into())
}

fn set_once(slot: &mut Option<String>, value: String, option: &str) -> io::Result<()> {
    if slot.is_some() {
        return Err(invalid_input(format!(
            "{option} may only be specified once"
        )));
    }
    *slot = Some(value);
    Ok(())
}

fn parse_count(value: Option<&str>, option: &str, default: usize) -> io::Result<usize> {
    value.map_or(Ok(default), |value| {
        value
            .parse()
            .map_err(|_| invalid_input(format!("{option} must be a non-negative integer")))
    })
}

fn print_help() {
    println!(
        "Usage: test_benchmark --protocol <tcp|raknet> --type <server|client> --address <IP:PORT> [--packets N] [--payload-size BYTES] [--warmup N] [--latency-samples N] [--raknet-mtu BYTES]\n\n\
         Client defaults: --packets {DEFAULT_PACKET_COUNT}, --payload-size {DEFAULT_PAYLOAD_SIZE}, --warmup {DEFAULT_WARMUP_COUNT}, --latency-samples {DEFAULT_LATENCY_SAMPLES}.\n\
         The client measures request/echo RTT samples, then pipelined echo throughput.\n\
         For TCP, each record is length-prefixed. RakNet uses ReliableOrdered packets."
    );
}

fn parse_args() -> io::Result<Config> {
    let mut protocol = None;
    let mut mode = None;
    let mut address = None;
    let mut packet_count = None;
    let mut payload_size = None;
    let mut warmup_count = None;
    let mut latency_samples = None;
    let mut raknet_mtu = None;
    let mut args = std::env::args().skip(1);

    while let Some(option) = args.next() {
        if option == "-h" || option == "--help" {
            print_help();
            std::process::exit(0);
        }

        let value = args
            .next()
            .ok_or_else(|| invalid_input(format!("missing value for {option}")))?;
        match option.as_str() {
            "-p" | "--protocol" => set_once(&mut protocol, value, &option)?,
            "-t" | "--type" => set_once(&mut mode, value, &option)?,
            "-a" | "--address" => set_once(&mut address, value, &option)?,
            "-n" | "--packets" => set_once(&mut packet_count, value, &option)?,
            "--payload-size" => set_once(&mut payload_size, value, &option)?,
            "--warmup" => set_once(&mut warmup_count, value, &option)?,
            "--latency-samples" => set_once(&mut latency_samples, value, &option)?,
            "--raknet-mtu" => set_once(&mut raknet_mtu, value, &option)?,
            _ => return Err(invalid_input(format!("unknown option: {option}"))),
        }
    }

    let protocol = match protocol.as_deref() {
        Some("tcp") => Protocol::Tcp,
        Some("raknet") => Protocol::RakNet,
        Some(value) => return Err(invalid_input(format!("unsupported protocol: {value}"))),
        None => return Err(invalid_input("--protocol is required")),
    };
    let mode = match mode.as_deref() {
        Some("client") => Mode::Client,
        Some("server") => Mode::Server,
        Some(value) => return Err(invalid_input(format!("unsupported type: {value}"))),
        None => return Err(invalid_input("--type is required")),
    };
    let address = address.ok_or_else(|| invalid_input("--address is required"))?;
    let packet_count = parse_count(packet_count.as_deref(), "--packets", DEFAULT_PACKET_COUNT)?;
    let payload_size = parse_count(
        payload_size.as_deref(),
        "--payload-size",
        DEFAULT_PAYLOAD_SIZE,
    )?;
    let warmup_count = parse_count(warmup_count.as_deref(), "--warmup", DEFAULT_WARMUP_COUNT)?;
    let latency_samples = parse_count(
        latency_samples.as_deref(),
        "--latency-samples",
        DEFAULT_LATENCY_SAMPLES,
    )?;

    let raknet_mtu = u16::try_from(parse_count(raknet_mtu.as_deref(), "--raknet-mtu", 1400)?)
        .map_err(|_| invalid_input("--raknet-mtu must be between 61 and 1492 bytes"))?;
    if !(61..=1492).contains(&raknet_mtu) {
        return Err(invalid_input(
            "--raknet-mtu must be between 61 and 1492 bytes",
        ));
    }

    if packet_count == 0 {
        return Err(invalid_input("--packets must be greater than zero"));
    }
    if payload_size == 0 || payload_size > MAX_PAYLOAD_SIZE {
        return Err(invalid_input(format!(
            "--payload-size must be between 1 and {MAX_PAYLOAD_SIZE} bytes"
        )));
    }
    if packet_count.checked_mul(payload_size).is_none() {
        return Err(invalid_input(
            "packet count times payload size is too large",
        ));
    }

    Ok(Config {
        protocol,
        mode,
        address,
        packet_count,
        payload_size,
        warmup_count,
        latency_samples,
        raknet_mtu,
    })
}

fn ensure_echo(actual: &[u8], expected: &[u8]) -> io::Result<()> {
    if actual == expected {
        Ok(())
    } else {
        Err(io::Error::new(
            ErrorKind::InvalidData,
            "echoed payload differs from the sent payload",
        ))
    }
}

fn encode_tcp_record(payload: &[u8]) -> Vec<u8> {
    let mut record = Vec::with_capacity(TCP_RECORD_HEADER_SIZE + payload.len());
    record.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    record.extend_from_slice(payload);
    record
}

async fn read_tcp_record<R: AsyncRead + Unpin>(
    reader: &mut R,
    expected_payload: &[u8],
    response: &mut Vec<u8>,
) -> io::Result<()> {
    let mut header = [0; TCP_RECORD_HEADER_SIZE];
    reader.read_exact(&mut header).await?;
    let payload_size = u32::from_be_bytes(header) as usize;
    if payload_size != expected_payload.len() {
        return Err(io::Error::new(
            ErrorKind::InvalidData,
            format!(
                "expected {} echoed bytes, received {payload_size}",
                expected_payload.len()
            ),
        ));
    }

    response.resize(payload_size, 0);
    reader.read_exact(response).await?;
    ensure_echo(response, expected_payload)
}

async fn tcp_round_trip(
    client: &mut TcpStream,
    record: &[u8],
    payload: &[u8],
    response: &mut Vec<u8>,
) -> io::Result<()> {
    client.write_all(record).await?;
    read_tcp_record(client, payload, response).await
}

async fn echo_tcp_client(mut client: TcpStream) -> io::Result<()> {
    client.set_nodelay(true)?;
    let mut record = Vec::new();
    loop {
        let mut header = [0; TCP_RECORD_HEADER_SIZE];
        match client.read_exact(&mut header).await {
            Ok(_) => {}
            Err(error) if error.kind() == ErrorKind::UnexpectedEof => return Ok(()),
            Err(error) => return Err(error),
        }

        let payload_size = u32::from_be_bytes(header) as usize;
        if payload_size > MAX_PAYLOAD_SIZE {
            return Err(invalid_input(format!(
                "TCP record exceeds the {MAX_PAYLOAD_SIZE}-byte benchmark limit"
            )));
        }

        record.resize(TCP_RECORD_HEADER_SIZE + payload_size, 0);
        record[..TCP_RECORD_HEADER_SIZE].copy_from_slice(&header);
        client
            .read_exact(&mut record[TCP_RECORD_HEADER_SIZE..])
            .await?;
        client.write_all(&record).await?;
    }
}

async fn run_tcp_client(config: &Config) -> Result<(), Box<dyn Error + Send + Sync>> {
    let mut client = TcpStream::connect(&config.address).await?;
    client.set_nodelay(true)?;

    let payload = vec![0xfe; config.payload_size];
    let record = encode_tcp_record(&payload);
    let mut response = Vec::with_capacity(config.payload_size);

    for _ in 0..config.warmup_count {
        tcp_round_trip(&mut client, &record, &payload, &mut response).await?;
    }

    let mut latencies = Vec::with_capacity(config.latency_samples);
    for _ in 0..config.latency_samples {
        let started = Instant::now();
        tcp_round_trip(&mut client, &record, &payload, &mut response).await?;
        latencies.push(started.elapsed());
    }

    let started = Instant::now();
    let (mut reader, mut writer) = client.into_split();
    let window = tokio::sync::Semaphore::new(64);
    let send_burst = async {
        for _ in 0..config.packet_count {
            let permit = window
                .acquire()
                .await
                .expect("benchmark window remains open");
            writer.write_all(&record).await?;
            permit.forget();
        }
        Ok::<(), io::Error>(())
    };
    let receive_burst = async {
        for _ in 0..config.packet_count {
            read_tcp_record(&mut reader, &payload, &mut response).await?;
            window.add_permits(1);
        }
        Ok::<(), io::Error>(())
    };
    tokio::try_join!(send_burst, receive_burst)?;
    let elapsed = started.elapsed();

    print_summary(config, &latencies, elapsed);
    Ok(())
}

async fn run_tcp_server(address: &str) -> Result<(), Box<dyn Error + Send + Sync>> {
    let listener = TcpListener::bind(address).await?;
    println!("TCP echo benchmark listening on {address}");

    loop {
        let (client, peer) = listener.accept().await?;
        tokio::spawn(async move {
            if let Err(error) = echo_tcp_client(client).await {
                eprintln!("TCP benchmark connection from {peer} ended: {error}");
            }
        });
    }
}

async fn raknet_round_trip(
    client: &RaknetSocket,
    payload: &[u8],
) -> Result<(), Box<dyn Error + Send + Sync>> {
    client.send(payload, Reliability::ReliableOrdered).await?;
    let response = client.recv().await?;
    ensure_echo(&response, payload)?;
    Ok(())
}

async fn run_raknet_client(config: &Config) -> Result<(), Box<dyn Error + Send + Sync>> {
    let address: SocketAddr = config.address.parse()?;
    let client =
        RaknetSocket::connect_with_version_and_mtu(&address, 10, config.raknet_mtu).await?;
    let payload = vec![0xfe; config.payload_size];

    for _ in 0..config.warmup_count {
        raknet_round_trip(&client, &payload).await?;
    }

    let mut latencies = Vec::with_capacity(config.latency_samples);
    for _ in 0..config.latency_samples {
        let started = Instant::now();
        raknet_round_trip(&client, &payload).await?;
        latencies.push(started.elapsed());
    }

    let started = Instant::now();
    let window = tokio::sync::Semaphore::new(64);
    let send_burst = async {
        for _ in 0..config.packet_count {
            let permit = window
                .acquire()
                .await
                .expect("benchmark window remains open");
            client.send(&payload, Reliability::ReliableOrdered).await?;
            permit.forget();
        }
        Ok::<(), Box<dyn Error + Send + Sync>>(())
    };
    let receive_burst = async {
        for _ in 0..config.packet_count {
            let response = client.recv().await?;
            ensure_echo(&response, &payload)?;
            window.add_permits(1);
        }
        Ok::<(), Box<dyn Error + Send + Sync>>(())
    };
    tokio::try_join!(send_burst, receive_burst)?;
    let elapsed = started.elapsed();

    print_summary(config, &latencies, elapsed);
    Ok(())
}

async fn run_raknet_server(
    address: &str,
    maximum_mtu: u16,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    let address: SocketAddr = address.parse()?;
    let mut listener = RaknetListener::bind_with_maximum_mtu(&address, maximum_mtu).await?;
    listener.listen().await;
    println!("RakNet echo benchmark listening on {address}");

    loop {
        let client = listener.accept().await?;
        tokio::spawn(async move {
            loop {
                let payload = match client.recv().await {
                    Ok(payload) => payload,
                    Err(_) => break,
                };
                if client
                    .send(&payload, Reliability::ReliableOrdered)
                    .await
                    .is_err()
                {
                    break;
                }
            }
        });
    }
}

fn percentile(sorted: &[Duration], percent: usize) -> Duration {
    let index = sorted
        .len()
        .saturating_mul(percent)
        .div_ceil(100)
        .saturating_sub(1)
        .min(sorted.len() - 1);
    sorted[index]
}

fn print_summary(config: &Config, latencies: &[Duration], elapsed: Duration) {
    let seconds = elapsed.as_secs_f64().max(f64::MIN_POSITIVE);
    let payload_bytes = config.packet_count * config.payload_size;

    println!("Protocol: {}", config.protocol.name());
    println!("Packets: {}", config.packet_count);
    println!("Payload size: {} bytes", config.payload_size);
    println!("Warmup rounds: {}", config.warmup_count);
    println!("RTT samples: {}", latencies.len());
    println!("Elapsed: {:.3} s", seconds);
    println!(
        "Echo payload throughput (per direction): {:.2} MiB/s",
        payload_bytes as f64 / 1_048_576.0 / seconds
    );
    println!(
        "Completed echo rate: {:.0} packets/s",
        config.packet_count as f64 / seconds
    );

    if latencies.is_empty() {
        println!("RTT: no samples (set --latency-samples to collect them)");
        return;
    }

    let mut sorted = latencies.to_vec();
    sorted.sort_unstable();
    let average_micros =
        sorted.iter().map(Duration::as_secs_f64).sum::<f64>() * 1_000_000.0 / sorted.len() as f64;
    println!("RTT average: {average_micros:.1} us");
    println!(
        "RTT p50: {:.1} us",
        percentile(&sorted, 50).as_secs_f64() * 1_000_000.0
    );
    println!(
        "RTT p95: {:.1} us",
        percentile(&sorted, 95).as_secs_f64() * 1_000_000.0
    );
    println!(
        "RTT p99: {:.1} us",
        percentile(&sorted, 99).as_secs_f64() * 1_000_000.0
    );
    println!("RTT min: {:.1} us", sorted[0].as_secs_f64() * 1_000_000.0);
    println!(
        "RTT max: {:.1} us",
        sorted[sorted.len() - 1].as_secs_f64() * 1_000_000.0
    );
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error + Send + Sync>> {
    if std::env::var_os("RAKNET_DEBUG").is_some() {
        rust_raknet::enable_raknet_log(7);
    }
    let config = parse_args()?;
    tokio::spawn(async move {
        match (config.protocol, config.mode) {
            (Protocol::Tcp, Mode::Client) => run_tcp_client(&config).await,
            (Protocol::Tcp, Mode::Server) => run_tcp_server(&config.address).await,
            (Protocol::RakNet, Mode::Client) => run_raknet_client(&config).await,
            (Protocol::RakNet, Mode::Server) => {
                run_raknet_server(&config.address, config.raknet_mtu).await
            }
        }
    })
    .await?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn tcp_echo_reuses_records_across_payload_sizes() {
        tokio::time::timeout(Duration::from_secs(2), async {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
            let echo = tokio::spawn(async move {
                let (socket, _) = listener.accept().await.unwrap();
                echo_tcp_client(socket).await.unwrap();
            });
            let mut client = TcpStream::connect(address).await.unwrap();
            client.set_nodelay(true).unwrap();
            let mut response = Vec::new();
            for size in [0, 64, 800, 4096, 64] {
                let payload: Vec<_> = (0..size).map(|index| index as u8).collect();
                client
                    .write_all(&encode_tcp_record(&payload))
                    .await
                    .unwrap();
                read_tcp_record(&mut client, &payload, &mut response)
                    .await
                    .unwrap();
                assert_eq!(response, payload);
            }
            drop(client);
            echo.await.unwrap();
        })
        .await
        .expect("TCP echo stalled");
    }
}
