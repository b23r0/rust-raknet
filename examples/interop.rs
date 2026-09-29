//! Independent-peer validation driver; run only inside an isolated network.
use rust_raknet::{RaknetSocket, Reliability};
use std::{error::Error, time::Duration};

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    if std::env::var_os("RAKNET_DEBUG").is_some() {
        rust_raknet::enable_raknet_log(7);
    }
    let address = std::env::args()
        .nth(1)
        .ok_or("missing peer address")?
        .parse()?;
    tokio::time::timeout(Duration::from_secs(60), async {
        let socket = RaknetSocket::connect_with_version(&address, 11).await?;
        let mut count = 0;
        for size in [64, 800, 4096, 10000] {
            for index in 0..100 {
                let mut payload: Vec<_> = (0..size).map(|i| (i + index) as u8).collect();
                payload[0] = 0xfe;
                socket.send(&payload, Reliability::ReliableOrdered).await?;
                assert_eq!(
                    socket.recv().await?,
                    payload,
                    "message reordered, duplicated or corrupted"
                );
                count += 1;
            }
        }
        socket.close().await?;
        println!("PASS: {count} unique ordered messages, 64/800/4096/10000 bytes");
        Ok::<_, Box<dyn Error>>(())
    })
    .await?
}
