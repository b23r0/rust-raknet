use std::collections::HashMap;
use tokio::sync::mpsc::channel;
use tokio::sync::Mutex;
use tokio::sync::mpsc::{Receiver, Sender};
use std::time::Duration;
use std::{io::Result, net::SocketAddr, sync::Arc};
use tokio::net::UdpSocket;
use tokio::time::sleep;
use rand;
use std::sync::atomic::{AtomicBool, Ordering};
use crate::socket::*;
use crate::packet::*;
use crate::utils::*;

const RAKNET_TIMEOUT: u64 = 30;
const SERVER_NAME : &str = "Rust Raknet Server";
const MAX_CONNECTION : u64 = 99999;

pub struct RaknetListener {
    motd : String,
    socket : Arc<UdpSocket>,
    guid : u64,
    connection_receiver : Receiver<Arc<RaknetSocket>>,
    connection_sender : Sender<Arc<RaknetSocket>>,
    connected : Arc<Mutex<HashMap<SocketAddr , Arc<RaknetSocket>>>>,
    pre_connection_next_status : Arc<Mutex<HashMap<SocketAddr , (PacketID, i64)>>>,
    pre_connection_next_status_check_opened : Arc<AtomicBool>
}

impl Drop for RaknetListener {
    fn drop(&mut self){
        self.pre_connection_next_status_check_opened.store(false, Ordering::Relaxed);
    }
}

impl RaknetListener {

    pub async fn bind(sockaddr : SocketAddr) -> Result<Self> {
        
        let s = match UdpSocket::bind(sockaddr).await{
            Ok(p) => p,
            Err(e) => {
                return Err(e);
            },
        };

        let pre_connection_next_status = Arc::new(Mutex::new(HashMap::new()));
        let (connection_sender ,connection_receiver) = channel::<Arc<RaknetSocket>>(10);

        Ok(Self {
            motd : String::new(),
            socket : Arc::new(s),
            guid : rand::random(),
            connection_receiver : connection_receiver,
            connection_sender : connection_sender,
            connected : Arc::new(Mutex::new(HashMap::new())),
            pre_connection_next_status : pre_connection_next_status,
            pre_connection_next_status_check_opened : Arc::new(AtomicBool::new(false))
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

        let pre_connection_next_status = Arc::new(Mutex::new(HashMap::new()));
        let (connection_sender ,connection_receiver) = channel::<Arc<RaknetSocket>>(10);
        
        Ok(Self {
            motd : String::new(),
            socket : Arc::new(s),
            guid : rand::random(),
            connection_receiver : connection_receiver,
            connection_sender : connection_sender,
            connected : Arc::new(Mutex::new(HashMap::new())),
            pre_connection_next_status : pre_connection_next_status,
            pre_connection_next_status_check_opened : Arc::new(AtomicBool::new(false))
        })
    }

    async fn start_check_pre_connection_timeout(&mut self){

        if self.pre_connection_next_status_check_opened.fetch_and(true , Ordering::Relaxed) {
            let flag = self.pre_connection_next_status_check_opened.clone();
            self.pre_connection_next_status_check_opened.store(true, Ordering::Relaxed);
            let pre_connection_next_status = self.pre_connection_next_status.clone();
            tokio::spawn(async move {
                if !flag.fetch_and(true , Ordering::Relaxed){
                    return;
                }

                sleep(Duration::from_secs(RAKNET_TIMEOUT)).await;
                
                let mut map = pre_connection_next_status.lock().await;
                map.retain(|_ , v| !is_timeout(v.1, RAKNET_TIMEOUT));
            });
        }
    }

    pub async fn listen(&mut self) {

        if self.motd.is_empty(){
            self.set_motd("486" , "1.18.11" ,self.guid, "Survival" , self.socket.local_addr().unwrap().port()).await;
        }

        self.start_check_pre_connection_timeout().await;

        let pre_connection_next_status = self.pre_connection_next_status.clone();
        let socket = self.socket.clone();
        let guid = self.guid.clone();
        let connected = self.connected.clone();
        let connection_sender = self.connection_sender.clone();
        let motd = self.get_motd().await;

        tokio::spawn(async move {
            let mut buf= [0u8;2048];
    
            loop{
                let motd = motd.clone();
                let (size , addr) = match socket.recv_from(&mut buf).await{
                    Ok(p) => p,
                    Err(_) => return,
                };
    
                let mut pre_connection_next_status = pre_connection_next_status.lock().await;
                let mut next_status : PacketID = PacketID::Unknown;
                let cur_status = transaction_packet_id(buf[0]);
    
                if pre_connection_next_status.contains_key(&addr){
                    let v = pre_connection_next_status.get_mut(&addr).unwrap();
                    next_status = v.0;
                    v.1 = cur_timestamp();
                }
                
                if cur_status == next_status || next_status == PacketID::Unknown{ 
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
                                use_encryption: 0x00, // Make sure this is false, it is vital for the login sequence to continue
                                mtu_size: req.mtu_size, // see Open Connection Request 1
                            };
                            
                            let reply = match write_packet_connection_open_reply_1(&packet).await{
                                Ok(p) => p,
                                Err(_) => continue,
                            };
    
                            socket.send_to(&reply, addr).await.unwrap();
    
                            pre_connection_next_status.insert(addr, (PacketID::OpenConnectionRequest2 , cur_timestamp()));
    
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
    
                            socket.send_to(&reply, addr).await.unwrap();
    
                            pre_connection_next_status.remove(&addr);
    
                            let s = Arc::new(RaknetSocket::from(&addr, &socket));
    
                            connected.lock().await.insert(addr, s.clone());
                            let _ = connection_sender.send(s).await;
                        },
                        _ => {
                            let connected = connected.lock().await;
                            if connected.contains_key(&addr){
                                connected[&addr].handle_packet(&buf[..size]);
                            }
                        },
                    }
                }
            }
            
        });
    }

    pub async fn accept(&mut self) -> Result<Arc<RaknetSocket>> {
        Ok(self.connection_receiver.recv().await.unwrap())
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