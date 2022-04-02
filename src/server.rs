use std::collections::HashMap;
use tokio::sync::mpsc::channel;
use tokio::sync::Mutex;
use tokio::sync::mpsc::{Receiver, Sender};
use std::{net::SocketAddr, sync::Arc};
use tokio::net::UdpSocket;

use crate::{socket::*, raknet_log};
use crate::packet::*;
use crate::utils::*;
use crate::error::{Result, RaknetError};

const SERVER_NAME : &str = "Rust Raknet Server";
const MAX_CONNECTION : u64 = 99999;

pub struct RaknetListener {
    motd : String,
    socket : Arc<UdpSocket>,
    guid : u64,
    listened : bool, 
    connection_receiver : Receiver<RaknetSocket>,
    connection_sender : Sender<RaknetSocket>,
    sessions : Arc<Mutex<HashMap<SocketAddr , (i64 ,Sender<Vec<u8>>)>>>
}

impl RaknetListener {

    pub async fn bind(sockaddr : SocketAddr) -> Result<Self> {
        
        let s = match UdpSocket::bind(sockaddr).await{
            Ok(p) => p,
            Err(_) => {
                return Err(RaknetError::BindAdreesError);
            },
        };

        let (connection_sender ,connection_receiver) = channel::<RaknetSocket>(10);

        Ok(Self {
            motd : String::new(),
            socket : Arc::new(s),
            guid : rand::random(),
            listened : false,
            connection_receiver,
            connection_sender,
            sessions : Arc::new(Mutex::new(HashMap::new())),
        })
    }
    
    pub async fn from_std(s : std::net::UdpSocket) -> Result<Self>{

        s.set_nonblocking(true).expect("set udpsocket nonblocking error");

        let s = match UdpSocket::from_std(s){
            Ok(p) => p,
            Err(_) => {
                return Err(RaknetError::SetRaknetRawSocketError);
            },
        };

        let (connection_sender ,connection_receiver) = channel::<RaknetSocket>(10);
        
        Ok(Self {
            motd : String::new(),
            socket : Arc::new(s),
            guid : rand::random(),
            listened : false,
            connection_receiver,
            connection_sender,
            sessions : Arc::new(Mutex::new(HashMap::new())),
        })
    }

    async fn start_session_collect(&self ,socket : &Arc<UdpSocket> , sessions : &Arc<Mutex<HashMap<SocketAddr , (i64 ,Sender<Vec<u8>>)>>> ,mut collect_receiver : Receiver<SocketAddr>) {
        let sessions = sessions.clone();
        let socket = socket.clone();
        tokio::spawn(async move{
            loop{
                let addr = match collect_receiver.recv().await{
                    Some(p) => p,
                    None => {
                        raknet_log!("session collecter closed");
                        break;
                    },
                };

                let mut sessions = sessions.lock().await;
                if sessions.contains_key(&addr){
                    socket.send_to(&[PacketID::Disconnect.to_u8()], addr).await.unwrap();
                    sessions.remove(&addr);
                    raknet_log!("collect socket : {}" , addr);
                }
            }
        });
    }

    pub async fn listen(&mut self) {

        if self.motd.is_empty(){
            self.set_motd("486" , "1.18.11" ,self.guid, "Survival" , self.socket.local_addr().unwrap().port()).await;
        }

        let socket = self.socket.clone();
        let guid = self.guid;
        let sessions = self.sessions.clone();
        let connection_sender = self.connection_sender.clone();
        let motd = self.get_motd().await;

        self.listened = true;

        let (collect_sender ,collect_receiver) = channel::<SocketAddr>(10);
        let collect_sender = Arc::new(Mutex::new(collect_sender));
        self.start_session_collect(&socket ,&sessions , collect_receiver).await;

        let local_addr = socket.local_addr().unwrap();

        tokio::spawn(async move {
            let mut buf= [0u8;2048];

            raknet_log!("start listen worker : {}" , local_addr );
    
            loop{
                let motd = motd.clone();
                let (size , addr) = match socket.recv_from(&mut buf).await{
                    Ok(p) => p,
                    Err(e) => {
                        raknet_log!("server recv_from error {}" , e);
                        break;
                    },
                };

                let cur_status = PacketID::from(buf[0]).unwrap();
                
                match cur_status{
                    PacketID::UnconnectedPing1 => {
                        let _ping = match read_packet_ping(&buf[..size]).await{
                            Ok(p) => p,
                            Err(_) => continue,
                        };

                        let packet = crate::packet::PacketUnconnectedPong { 
                            time: cur_timestamp_millis(), 
                            guid, 
                            magic: true, 
                            motd 
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
                            time: cur_timestamp_millis(), 
                            guid, 
                            magic: true, 
                            motd
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
                            guid,
                            // Make sure this is false, it is vital for the login sequence to continue
                            use_encryption: 0x00, 
                            // see Open Connection Request 1
                            mtu_size: RAKNET_CLIENT_MTU, 
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
                            guid,
                            address: addr,
                            mtu: req.mtu,
                            encryption_enabled: 0x00,
                        };
                        
                        let reply = match write_packet_connection_open_reply_2(&packet).await{
                            Ok(p) => p,
                            Err(_) => continue,
                        };

                        let mut sessions = sessions.lock().await;

                        if sessions.contains_key(&addr) {

                            let packet = write_packet_already_connected(&AlreadyConnected{
                                magic: true,
                                guid,
                            }).await.unwrap();

                            socket.send_to(&packet, addr).await.unwrap();

                            continue;
                        }

                        socket.send_to(&reply, addr).await.unwrap();

                        let (sender , receiver) = channel::<Vec<u8>>(10);

                        let s = RaknetSocket::from(&addr, &socket, receiver , req.mtu , collect_sender.clone());

                        raknet_log!("accept connection : {}", addr);
                        sessions.insert(addr, (cur_timestamp_millis() ,sender));
                        let _ = connection_sender.send(s).await;
                    },
                    PacketID::Disconnect => {
                        let mut sessions = sessions.lock().await;
                        if sessions.contains_key(&addr){
                            sessions[&addr].1.send(buf[..size].to_vec()).await.unwrap();
                            sessions.remove(&addr);
                        }
                    }
                    _ => {
                        let mut sessions = sessions.lock().await;
                        if sessions.contains_key(&addr){
                            match sessions[&addr].1.send(buf[..size].to_vec()).await{
                                Ok(_) => {},
                                Err(_) => {
                                    sessions.remove(&addr);
                                    continue;
                                },
                            };
                            sessions.get_mut(&addr).unwrap().0 = cur_timestamp_millis();
                        }
                    },
                }
            }
            raknet_log!("listen worker closed");
        });
    }

    pub async fn accept(&mut self) -> Result<RaknetSocket> {
        if !self.listened{
            Err(RaknetError::NotListen)
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