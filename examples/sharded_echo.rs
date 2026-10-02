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
    let mut batch_messages = false;
    for option in std::env::args().skip(3) {
        listener = match option.as_str() {
            "--receive-batching" => listener.with_receive_batching(true),
            "--idle-maintenance" => listener.with_idle_maintenance(true),
            "--batch-messages" => {
                batch_messages = true;
                listener
            }
            _ => return Err(format!("unknown option: {option}").into()),
        };
    }
    listener.listen().await;
    println!("Sharded echo listening on {}", listener.local_addr()?);
    loop {
        let client = listener.accept().await?;
        if batch_messages {
            tokio::spawn(async move {
                let mut messages = Vec::with_capacity(16);
                while client.recv_bytes_batch(&mut messages, 16).await.is_ok() {
                    if client
                        .send_bytes_batch(&messages, Reliability::ReliableOrdered)
                        .await
                        .is_err()
                    {
                        break;
                    }
                    if messages.len() == 16 {
                        tokio::task::yield_now().await;
                    }
                }
            });
        } else {
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
}

#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("Socket sharding requires Linux");
}
