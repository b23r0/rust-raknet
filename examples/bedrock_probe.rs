use rust_raknet::{RaknetSocket, Reliability};
use std::{env, time::Duration};
#[tokio::main]
async fn main() {
    let args: Vec<_> = env::args().collect();
    let address = args[1].parse().unwrap();
    let version: u8 = args[2].parse().unwrap();
    rust_raknet::enable_raknet_log(3);
    println!("TARGET {address} RAKNET {version}");
    let socket = match tokio::time::timeout(
        Duration::from_secs(10),
        RaknetSocket::connect_with_version(&address, version),
    )
    .await
    {
        Ok(Ok(socket)) => {
            println!("HANDSHAKE OK");
            socket
        }
        Ok(Err(error)) => {
            println!("HANDSHAKE ERROR {error}");
            std::process::exit(1);
        }
        Err(_) => {
            println!("HANDSHAKE TIMEOUT");
            std::process::exit(2);
        }
    };
    if let Some(protocol) = args.get(3) {
        let protocol: i32 = protocol.parse().unwrap();
        let mut request = vec![0xfe, 6, 0xc1, 1];
        request.extend_from_slice(&protocol.to_be_bytes());
        socket
            .send(&request, Reliability::ReliableOrdered)
            .await
            .unwrap();
        println!("SENT RequestNetworkSettings protocol={protocol}");
        match tokio::time::timeout(Duration::from_secs(5), socket.recv()).await {
            Ok(Ok(data)) => {
                println!(
                    "RECEIVED {} bytes: {:02x?}",
                    data.len(),
                    &data[..data.len().min(80)]
                );
                if data.len() >= 8 && data[0] == 0xfe && data[2..4] == [0x8f, 1] {
                    println!(
                        "NETWORK_SETTINGS OK threshold={} algorithm={}",
                        u16::from_le_bytes([data[4], data[5]]),
                        u16::from_le_bytes([data[6], data[7]])
                    );
                } else {
                    println!("UNEXPECTED APPLICATION RESPONSE");
                    std::process::exit(3);
                }
            }
            other => {
                println!("APPLICATION RECEIVE ERROR {other:?}");
                std::process::exit(4);
            }
        }
    }
    let _ = tokio::time::timeout(Duration::from_secs(1), socket.close()).await;
}
