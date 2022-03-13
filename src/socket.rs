use std::{io::Result , net::{SocketAddr}, sync::Arc};
use tokio::net::UdpSocket;

use crate::{packet::{self, PacketUnconnectedPing, write_packet_ping, read_packet_pong}, utils::cur_timestamp};

pub struct RaknetSocket{
    addr : SocketAddr,
    s : Arc<UdpSocket>
}

impl RaknetSocket {
    pub fn new(addr : &SocketAddr , s : &Arc<UdpSocket>) -> Self {
        Self{
            addr : addr.clone(),
            s: s.clone(),
        }
    }

    pub async fn ping(addr : &SocketAddr) -> Result<i64> {
        let packet = PacketUnconnectedPing{
            time: cur_timestamp(),
            magic: true,
            guid: rand::random(),
        };

        let s = match UdpSocket::bind("0.0.0.0:0").await{
            Ok(p) => p,
            Err(e) => return Err(e),
        };

        let buf = write_packet_ping(&packet).await.unwrap();

        s.send_to(buf.as_slice(), addr).await.unwrap();

        let mut buf = [0u8 ; 1024];
        match s.recv_from(&mut buf).await{
            Ok(p) => p,
            Err(e) => return Err(e),
        };

        let pong = match read_packet_pong(&buf).await{
            Ok(p) => p,
            Err(e) => return Err(e),
        };

        Ok(pong.time - packet.time)
    }
}