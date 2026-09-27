use std::{
    error::Error,
    io::{self, ErrorKind},
    net::SocketAddr,
    time::{Duration, Instant},
};

use rust_raknet::{RaknetListener, RaknetSocket, Reliability};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    time::sleep,
};

const PACKET_COUNT: usize = 100;
const PAYLOAD_SIZE: usize = 800;

#[derive(Clone, Copy)]
enum Protocol {
    Tcp,
    RakNet,
}

#[derive(Clone, Copy)]
enum Mode {
    Client,
    Server,
}

fn invalid_input(message: impl Into<String>) -> io::Error {
    io::Error::new(ErrorKind::InvalidInput, message.into())
}

fn parse_args() -> Result<(Protocol, Mode, String), Box<dyn Error>> {
    let mut protocol = None;
    let mut mode = None;
    let mut address = None;
    let mut args = std::env::args().skip(1);

    while let Some(option) = args.next() {
        if option == "-h" || option == "--help" {
            println!("Usage: test_benchmark --protocol <tcp|raknet> --type <server|client> --address <IP:PORT>");
            std::process::exit(0);
        }

        let value = args
            .next()
            .ok_or_else(|| invalid_input(format!("missing value for {option}")))?;
        let destination = match option.as_str() {
            "-p" | "--protocol" => &mut protocol,
            "-t" | "--type" => &mut mode,
            "-a" | "--address" => &mut address,
            _ => return Err(invalid_input(format!("unknown option: {option}")).into()),
        };
        if destination.replace(value).is_some() {
            return Err(invalid_input(format!("{option} may only be specified once")).into());
        }
    }

    let protocol = match protocol.as_deref() {
        Some("tcp") => Protocol::Tcp,
        Some("raknet") => Protocol::RakNet,
        Some(value) => return Err(invalid_input(format!("unsupported protocol: {value}")).into()),
        None => return Err(invalid_input("--protocol is required").into()),
    };
    let mode = match mode.as_deref() {
        Some("client") => Mode::Client,
        Some("server") => Mode::Server,
        Some(value) => return Err(invalid_input(format!("unsupported type: {value}")).into()),
        None => return Err(invalid_input("--type is required").into()),
    };
    let address = address.ok_or_else(|| invalid_input("--address is required"))?;

    Ok((protocol, mode, address))
}

fn print_latency_summary(latencies: &[u128]) {
    let total: u128 = latencies.iter().sum();
    for latency in latencies {
        println!("latency: {latency} ms");
    }
    println!("average: {} ms", total / latencies.len() as u128);
}

async fn run_tcp_client(address: &str) -> Result<(), Box<dyn Error>> {
    let mut client = TcpStream::connect(address).await?;
    let mut latencies = Vec::with_capacity(PACKET_COUNT);
    let mut buffer = [0; PAYLOAD_SIZE];

    for _ in 0..PACKET_COUNT {
        let started = Instant::now();
        client.read_exact(&mut buffer).await?;
        latencies.push(started.elapsed().as_millis());
    }

    print_latency_summary(&latencies);
    Ok(())
}

async fn run_tcp_server(address: &str) -> Result<(), Box<dyn Error>> {
    let listener = TcpListener::bind(address).await?;
    loop {
        let (mut client, _) = listener.accept().await?;
        tokio::spawn(async move {
            let payload = [0; PAYLOAD_SIZE];
            for _ in 0..PACKET_COUNT {
                sleep(Duration::from_millis(30)).await;
                if client.write_all(&payload).await.is_err() {
                    break;
                }
            }
        });
    }
}

async fn run_raknet_client(address: &str) -> Result<(), Box<dyn Error>> {
    let address: SocketAddr = address.parse()?;
    let client = RaknetSocket::connect(&address).await?;
    let mut latencies = Vec::with_capacity(PACKET_COUNT);

    for _ in 0..PACKET_COUNT {
        let started = Instant::now();
        client.recv().await?;
        latencies.push(started.elapsed().as_millis());
    }

    print_latency_summary(&latencies);
    Ok(())
}

async fn run_raknet_server(address: &str) -> Result<(), Box<dyn Error>> {
    let address: SocketAddr = address.parse()?;
    let mut listener = RaknetListener::bind(&address).await?;
    listener.listen().await;

    loop {
        let client = listener.accept().await?;
        tokio::spawn(async move {
            let payload = [0xfe; PAYLOAD_SIZE];
            for _ in 0..PACKET_COUNT {
                sleep(Duration::from_millis(30)).await;
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

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let (protocol, mode, address) = parse_args()?;
    match (protocol, mode) {
        (Protocol::Tcp, Mode::Client) => run_tcp_client(&address).await,
        (Protocol::Tcp, Mode::Server) => run_tcp_server(&address).await,
        (Protocol::RakNet, Mode::Client) => run_raknet_client(&address).await,
        (Protocol::RakNet, Mode::Server) => run_raknet_server(&address).await,
    }
}
