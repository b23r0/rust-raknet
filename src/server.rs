use std::collections::HashMap;
use tokio::sync::mpsc::channel;
use tokio::sync::Mutex;
use tokio::sync::mpsc::{Receiver, Sender};
use std::{io::Result, net::SocketAddr, sync::Arc};
use tokio::net::UdpSocket;
use rand;
use crate::socket::*;
use crate::packet::*;
use crate::utils::*;

const SERVER_NAME : &str = "Rust Raknet Server";
const MAX_CONNECTION : u64 = 99999;

pub struct RaknetListener {
    motd : String,
    socket : Arc<UdpSocket>,
    guid : u64,
    listened : bool, 
    connection_receiver : Receiver<RaknetSocket>,
    connection_sender : Sender<RaknetSocket>,
    connected : Arc<Mutex<HashMap<SocketAddr , Sender<Vec<u8>>>>>
}

impl RaknetListener {

    pub async fn bind(sockaddr : SocketAddr) -> Result<Self> {
        
        let s = match UdpSocket::bind(sockaddr).await{
            Ok(p) => p,
            Err(e) => {
                return Err(e);
            },
        };

        let (connection_sender ,connection_receiver) = channel::<RaknetSocket>(10);

        Ok(Self {
            motd : String::new(),
            socket : Arc::new(s),
            guid : rand::random(),
            listened : false,
            connection_receiver : connection_receiver,
            connection_sender : connection_sender,
            connected : Arc::new(Mutex::new(HashMap::new())),
        })
    }
    
    pub async fn from_std(s : std::net::UdpSocket) -> Result<Self>{

        s.set_nonblocking(true).expect("set udpsocket nonblocking error");

        let s = match UdpSocket::from_std(s){
            Ok(p) => p,
            Err(e) => {
                return Err(e);
            },
        };

        let (connection_sender ,connection_receiver) = channel::<RaknetSocket>(10);
        
        Ok(Self {
            motd : String::new(),
            socket : Arc::new(s),
            guid : rand::random(),
            listened : false,
            connection_receiver : connection_receiver,
            connection_sender : connection_sender,
            connected : Arc::new(Mutex::new(HashMap::new())),
        })
    }

    pub async fn listen(&mut self) {

        if self.motd.is_empty(){
            self.set_motd("486" , "1.18.11" ,self.guid, "Survival" , self.socket.local_addr().unwrap().port()).await;
        }

        let socket = self.socket.clone();
        let guid = self.guid.clone();
        let connected = self.connected.clone();
        let connection_sender = self.connection_sender.clone();
        let motd = self.get_motd().await;

        self.listened = true;

        tokio::spawn(async move {
            let mut buf= [0u8;2048];
    
            loop{
                let motd = motd.clone();
                let (size , addr) = match socket.recv_from(&mut buf).await{
                    Ok(p) => p,
                    Err(_) => return,
                };

                let cur_status = PacketID::from(buf[0]);
                
                match cur_status{
                    PacketID::UnconnectedPing1 => {
                        let _ping = match read_packet_ping(&buf[..size]).await{
                            Ok(p) => p,
                            Err(_) => continue,
                        };

                        let packet = crate::packet::PacketUnconnectedPong { 
                            time: cur_timestamp(), 
                            guid: guid, 
                            magic: true, 
                            motd: motd 
                        };
                        
                        let pong = match write_packet_pong(&packet).await{
                            Ok(p) => p,
                            Err(_) => continue,
                        };

                        socket.send_to(&pong, addr).await.unwrap();
                        continue;
                    },
                    PacketID::UnconnectedPing2 => {
                        match read_packet_ping(&buf[..size]).await{
                            Ok(p) => p,
                            Err(_) => continue,
                        };

                        let packet = crate::packet::PacketUnconnectedPong { 
                            time: cur_timestamp(), 
                            guid: guid, 
                            magic: true, 
                            motd: motd
                        };
                        
                        let pong = match write_packet_pong(&packet).await{
                            Ok(p) => p,
                            Err(_) => continue,
                        };

                        socket.send_to(&pong, addr).await.unwrap();
                        continue;
                    },
                    PacketID::OpenConnectionRequest1 => {
                        let req = match read_packet_connection_open_request_1(&buf[..size]).await{
                            Ok(p) => p,
                            Err(_) => continue,
                        };

                        if req.protocol_version != RAKNET_PROTOCOL_VERSION{
                            let packet = crate::packet::IncompatibleProtocolVersion{
                                server_protocol: RAKNET_PROTOCOL_VERSION,
                                magic: true,
                                server_guid: guid,
                            };
                            let buf = write_packet_incompatible_protocol_version(&packet).await.unwrap();

                            socket.send_to(&buf, addr).await.unwrap();
                            continue;
                        }

                        let packet = crate::packet::OpenConnectionReply1 {
                            magic: true,
                            guid: guid,
                            // Make sure this is false, it is vital for the login sequence to continue
                            use_encryption: 0x00, 
                            // see Open Connection Request 1
                            mtu_size: req.mtu_size, 
                        };
                        
                        let reply = match write_packet_connection_open_reply_1(&packet).await{
                            Ok(p) => p,
                            Err(_) => continue,
                        };

                        socket.send_to(&reply, addr).await.unwrap();
                        continue;
                    },
                    PacketID::OpenConnectionRequest2 => {
                        let req = match read_packet_connection_open_request_2(&buf[..size]).await{
                            Ok(p) => p,
                            Err(_) => continue,
                        };

                        let packet = crate::packet::OpenConnectionReply2 {
                            magic: true,
                            guid: guid,
                            address: addr,
                            mtu: req.mtu,
                            encryption_enabled: 0x00,
                        };
                        
                        let reply = match write_packet_connection_open_reply_2(&packet).await{
                            Ok(p) => p,
                            Err(_) => continue,
                        };

                        let mut connected = connected.lock().await;

                        if connected.contains_key(&addr) {

                            let packet = write_packet_already_connected(&AlreadyConnected{
                                magic: true,
                                guid : guid,
                            }).await.unwrap();

                            socket.send_to(&packet, addr).await.unwrap();

                            continue;
                        }

                        socket.send_to(&reply, addr).await.unwrap();

                        let (sender , receiver) = channel::<Vec<u8>>(10);

                        let s = RaknetSocket::from(&addr, &socket, receiver , req.mtu);

                        connected.insert(addr, sender);
                        let _ = connection_sender.send(s).await;
                    },
                    PacketID::Disconnect => {
                        let mut connected = connected.lock().await;
                        if connected.contains_key(&addr){
                            connected[&addr].send(buf[..size].to_vec()).await.unwrap();
                            connected.remove(&addr);
                        }
                    }
                    _ => {
                        let connected = connected.lock().await;
                        if connected.contains_key(&addr){
                            connected[&addr].send(buf[..size].to_vec()).await.unwrap();
                        }
                    },
                }
            }
            
        });
    }

    pub async fn accept(&mut self) -> Result<RaknetSocket> {
        if !self.listened{
            Err(std::io::Error::new(std::io::ErrorKind::Other , "not listen"))
        }else {
            Ok(self.connection_receiver.recv().await.unwrap())
        }
    }

    pub async fn set_motd(&mut self , mc_protocol_version : &str , mc_version : &str , guid : u64 ,  game_type : &str ,port : u16 ) {
        self.motd = format!("MCPE;{};{};{};0;{};{};Bedrock level;{};1;{};",SERVER_NAME, mc_protocol_version , mc_version , MAX_CONNECTION , guid , game_type,  port);
    }

    pub async fn get_motd(&self) -> String{
        self.motd.clone()
    }

    pub fn local_addr(&self) -> Result<SocketAddr> {
        Ok(self.socket.local_addr().unwrap())
    }
}