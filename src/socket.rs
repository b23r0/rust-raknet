use std::{io::Result , net::{SocketAddr}, sync::{Arc}};
use tokio::{net::UdpSocket, sync::Mutex, time::sleep};
use rand;
use tokio::sync::mpsc::{Sender, Receiver};
use std::sync::atomic::{AtomicBool, Ordering};

use crate::{packet::*, utils::*, arq::{Frame, ACKSet}};

pub struct RaknetSocket{
    local_addr : SocketAddr,
    peer_addr : SocketAddr,
    s : Arc<UdpSocket>,
    sender : Option<Sender<Vec<u8>>>,
    connected : Arc<AtomicBool>,
    ackset : Arc<Mutex<ACKSet>>,
    _mtu : u16,
    _guid : u64
}

impl RaknetSocket {
    pub fn from(addr : &SocketAddr , s : &Arc<UdpSocket> ,receiver : Receiver<Vec<u8>>) -> Self {
        let ret = RaknetSocket{
            peer_addr : addr.clone(),
            local_addr : s.local_addr().unwrap(),
            s: s.clone(),
            sender : None,
            connected : Arc::new(AtomicBool::new(true)),
            ackset : Arc::new(Mutex::new(ACKSet::new())),
            _mtu : RAKNET_MTU,
            _guid : rand::random()
        };
        ret.start_receiver(receiver);
        ret.start_tick();
        ret
    }

    pub async fn connect(addr : &SocketAddr) -> Result<Self>{

        let guid : u64 = rand::random();

        let s = match UdpSocket::bind("0.0.0.0:0").await{
            Ok(p) => p,
            Err(e) => return Err(e),
        };

        let packet = OpenConnectionRequest1{
            magic: true,
            protocol_version: RAKNET_PROTOCOL_VERSION,
            mtu_size: RAKNET_MTU,
        };

        let buf = write_packet_connection_open_request_1(&packet).await.unwrap();

        s.send_to(&buf, addr).await.unwrap();

        let mut buf = [0u8 ; 2048];
        let (size ,src ) = s.recv_from(&mut buf).await.unwrap();

        if buf[0] != transaction_packet_id_to_u8(PacketID::OpenConnectionReply1){
            if buf[0] == transaction_packet_id_to_u8(PacketID::IncompatibleProtocolVersion){
                let packet = match read_packet_incompatible_protocol_version(&buf[..size]).await{
                    Ok(p) => p,
                    Err(e) => return Err(e),
                };

                return Err(std::io::Error::new(std::io::ErrorKind::Other, format!("server version : {}", packet.server_protocol)));
            }else{
                return Err(std::io::Error::new(std::io::ErrorKind::Other, "open connection reply2 packetid incorrect"));
            }
        }

        let reply1 = match read_packet_connection_open_reply_1(&buf[..size]).await{
            Ok(p) => p,
            Err(e) => return Err(e),
        };

        let packet = OpenConnectionRequest2{
            magic: true,
            address: src,
            mtu: reply1.mtu_size,
            guid: guid,
        };

        let buf = write_packet_connection_open_request_2(&packet).await.unwrap();

        s.send_to(&buf, addr).await.unwrap();

        let mut buf = [0u8 ; 2048];
        let (size ,_ ) = s.recv_from(&mut buf).await.unwrap();

        if buf[0] != transaction_packet_id_to_u8(PacketID::OpenConnectionReply2){
            if buf[0] == transaction_packet_id_to_u8(PacketID::IncompatibleProtocolVersion){

                let packet = match read_packet_incompatible_protocol_version(&buf[..size]).await{
                    Ok(p) => p,
                    Err(e) => return Err(e),
                };

                return Err(std::io::Error::new(std::io::ErrorKind::Other, format!("server only support protocol version : {}", packet.server_protocol)));
            }else{
                return Err(std::io::Error::new(std::io::ErrorKind::Other, "open connection reply2 packetid incorrect"));
            }
        }

        let _reply2 = match read_packet_connection_open_reply_2(&buf[..size]).await{
            Ok(p) => p,
            Err(e) => return Err(e),
        };

        Ok(RaknetSocket{
            peer_addr : addr.clone(),
            local_addr : s.local_addr().unwrap(),
            s: Arc::new(s),
            sender : None,
            connected : Arc::new(AtomicBool::new(true)),
            ackset : Arc::new(Mutex::new(ACKSet::new())),
            _mtu : RAKNET_MTU,
            _guid : guid
        })
    }

    fn start_receiver(&self , mut receiver : Receiver<Vec<u8>>) {
        let connected = self.connected.clone();
        let ackset = self.ackset.clone();
        let s = self.s.clone();
        let peer_addr = self.peer_addr.clone();
        tokio::spawn(async move {
            loop{
                match receiver.recv().await{
                    Some(buf) => {
                        match transaction_packet_id(buf[0]){
                            PacketID::Disconnect => {
                                connected.fetch_and(false, Ordering::Relaxed);
                                break;
                            },
                            _ => {
                                // handle packet in here
                                dbg!(buf.clone());
                                
                                if buf[0] >= transaction_packet_id_to_u8(PacketID::FrameSetPacketBegin) && 
                                   buf[0] <= transaction_packet_id_to_u8(PacketID::FrameSetPacketEnd) {

                                    let mut ackset = ackset.lock().await;
                                    let frame = Frame::deserialize(buf).await;
                                    ackset.insert(frame.sequence_number).await;
                                    for i in ackset.get_nack().await{
                                        let nack = NACK{
                                            record_count: 1,
                                            single_sequence_number: true,
                                            sequences: (i , i),
                                        };

                                        let buf = write_packet_nack(&nack).await.unwrap(); 
                                        s.send_to(&buf, peer_addr).await.unwrap();
                                    }
                                    
                                }
                            },
                        }
                        
                    },
                    None => {
                        connected.fetch_and(false, Ordering::Relaxed);
                        break;
                    },
                };  
            }
        });
    }

    fn start_tick(&self) {
        let connected = self.connected.clone();
        let ackset = self.ackset.clone();
        let s = self.s.clone();
        let peer_addr = self.peer_addr.clone();
        tokio::spawn(async move {
            loop{
                sleep(std::time::Duration::from_millis(50)).await;

                // flush ack
                if connected.fetch_and(true, Ordering::Relaxed){
                    let mut ackset = ackset.lock().await;
                    let acks = ackset.get_ack().await;
                    for ack in acks {

                        let record_count = 1;
                        let single_sequence_number = ack.1 == ack.0;
                        
                        let packet = ACK{
                            record_count: record_count,
                            single_sequence_number: single_sequence_number,
                            sequences: ack,
                        };

                        let buf = write_packet_ack(&packet).await.unwrap();
                        s.send_to(&buf, peer_addr);
                    }
                }else{
                    break;
                }
            }
        });
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

    pub async fn close(&mut self) -> Result<()>{
        match self.s.send_to(&[0x15], self.peer_addr).await{
            Ok(_) => {
                self.connected.fetch_and(false, Ordering::Relaxed);
                Ok(())
            },
            Err(_) => {
                self.connected.fetch_and(false, Ordering::Relaxed);
                Err(std::io::Error::new(std::io::ErrorKind::Other , "send disconnect message faild , but connection still closed"))
            },
        }
    }

    pub fn peer_addr(&self) -> Result<SocketAddr>{
        Ok(self.peer_addr)
    }

    pub fn local_addr(&self) -> Result<SocketAddr>{
        Ok(self.local_addr)
    }
}