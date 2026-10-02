use std::{env, error::Error, io, time::Duration};

use rust_raknet::RaknetSocket;

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let target = env::args()
        .nth(1)
        .unwrap_or_else(|| "127.0.0.1:19132".to_owned());
    let address = tokio::net::lookup_host(&target)
        .await?
        .next()
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no address resolved"))?;

    let (latency, motd) =
        tokio::time::timeout(Duration::from_secs(5), RaknetSocket::ping(&address)).await??;

    println!("Server: {address}");
    println!("Round trip: {latency} ms");
    println!("MOTD: {motd}");

    let fields: Vec<_> = motd.split(';').collect();
    for (label, index) in [
        ("Name", 1),
        ("Protocol version", 2),
        ("Game version", 3),
        ("Players", 4),
        ("Maximum players", 5),
        ("World", 7),
        ("Game mode", 8),
    ] {
        if let Some(value) = fields.get(index) {
            println!("{label}: {value}");
        }
    }

    Ok(())
}
