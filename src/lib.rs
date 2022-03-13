mod socket;
mod packet;
mod utils;
mod reader;
mod writer;
pub mod server;
pub use crate::server::*;

#[tokio::test]
async fn test_ping_pong(){

    let s = tokio::net::UdpSocket::bind("0.0.0.0:0").await.unwrap();
    let port = s.local_addr().unwrap().port();

    tokio::spawn(async move {
        let mut buf = [0u8 ; 1024];
        let (size , addr ) = s.recv_from(&mut buf).await.unwrap();

        let _pong = packet::read_packet_ping(&buf[..size]).await.unwrap();
        let packet = packet::PacketUnconnectedPong{
            time: utils::cur_timestamp(),
            magic: true,
            guid: rand::random(),
            motd : format!("MCPE;Dedicated Server;486;1.18.11;0;10;12322747879247233720;Bedrock level;Survival;1;{};", s.local_addr().unwrap().port())
        };

        let buf = packet::write_packet_pong(&packet).await.unwrap();

        s.send_to(buf.as_slice(), addr).await.unwrap();
    });

    let addr = format!("127.0.0.1:{}", port);
    let latency = socket::RaknetSocket::ping(&addr.as_str().parse().unwrap()).await.unwrap();
    assert!(latency < 10 && latency >= 0);
}

