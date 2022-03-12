use std::{net::{SocketAddr}, sync::Arc};
use tokio::net::UdpSocket;

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
}