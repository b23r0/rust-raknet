#[cfg(feature = "blocking")]
use rust_raknet::{Reliability, blocking::RaknetRuntime};
#[cfg(feature = "blocking")]
use std::time::Duration;

#[cfg(feature = "blocking")]
fn main() -> rust_raknet::blocking::Result<()> {
    let runtime = RaknetRuntime::new()?;
    let address = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "127.0.0.1:19132".into());
    let address = address.parse().expect("expected an IP address and port");
    let client = runtime.connect_timeout(&address, Duration::from_secs(10))?;
    client.send(&[0xfe, 42], Reliability::ReliableOrdered)?;
    println!("Echo: {:?}", client.recv_timeout(Duration::from_secs(5))?);
    client.flush_timeout(Duration::from_secs(5))?;
    client.close()
}

#[cfg(not(feature = "blocking"))]
fn main() {
    eprintln!(
        "Enable the blocking feature: cargo run --features blocking --example blocking_client"
    );
}
