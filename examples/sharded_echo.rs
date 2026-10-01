//! Echo server using a fixed Linux receive-socket group.
#[cfg(target_os = "linux")]
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    use rust_raknet::{RaknetListener, Reliability};
    let address = std::env::args().nth(1).ok_or("ADDRESS required")?.parse()?;
    let count = std::env::args()
        .nth(2)
        .unwrap_or_else(|| "4".into())
        .parse()?;
    let mut listener = RaknetListener::bind_with_socket_shards(&address, count).await?;
    listener.listen().await;
    println!("Sharded echo listening on {}", listener.local_addr()?);
    loop {
        let client = listener.accept().await?;
        tokio::spawn(async move {
            while let Ok(payload) = client.recv().await {
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

#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("Socket sharding requires Linux");
}
