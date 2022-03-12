use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;
use std::{io::Result, net::SocketAddr, sync::Arc};
use tokio::net::UdpSocket;
use tokio::time::sleep;
use rand;
use std::sync::atomic::{AtomicBool, Ordering};
use crate::socket::*;
use crate::packet::{PacketID, transaction_packet_id, read_packet_ping, write_packet_pong};
use crate::utils::*;

const RAKNET_TIMEOUT: u64 = 30;
const SERVER_NAME : &str = "Rust Raknet Server";
const MAX_CONNECTION : u64 = 99999;

pub struct RaknetListener {
    motd : String,
    socket : Arc<UdpSocket>,
    guid : u64,
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

        Ok(Self {
            motd : String::new(),
            socket : Arc::new(s),
            guid : rand::random(),
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
        
        Ok(Self {
            motd : String::new(),
            socket : Arc::new(s),
            guid : rand::random(),
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
                
                let mut map = pre_connection_next_status.lock().unwrap();
                map.retain(|_ , v| !is_timeout(v.1, RAKNET_TIMEOUT));
            });
        }
    }

    pub async fn accept(&mut self) -> Result<RaknetSocket> {
        if self.motd.is_empty(){
            self.set_motd("486" , "1.18.11" ,self.guid, "Survival" , self.socket.local_addr().unwrap().port()).await;
        }

        self.start_check_pre_connection_timeout().await;

        let mut buf= [0u8;2048];

        loop{
            let (size , addr) = match self.socket.recv_from(&mut buf).await{
                Ok(p) => p,
                Err(e) => return Err(e),
            };

            let mut pre_connection_next_status = self.pre_connection_next_status.lock().unwrap();
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
                            guid: self.guid, 
                            magic: true, 
                            motd: self.get_motd().await 
                        };
                        
                        let pong = match write_packet_pong(&packet).await{
                            Ok(p) => p,
                            Err(_) => continue,
                        };

                        self.socket.send_to(&pong, addr).await.unwrap();
                        continue;
                    },
                    PacketID::UnconnectedPing2 => {
                        match read_packet_ping(&buf[..size]).await{
                            Ok(p) => p,
                            Err(_) => continue,
                        };

                        let packet = crate::packet::PacketUnconnectedPong { 
                            time: cur_timestamp(), 
                            guid: self.guid, 
                            magic: true, 
                            motd: self.get_motd().await 
                        };
                        
                        let pong = match write_packet_pong(&packet).await{
                            Ok(p) => p,
                            Err(_) => continue,
                        };

                        self.socket.send_to(&pong, addr).await.unwrap();
                        continue;
                    },
                    PacketID::UnconnectedPong => todo!(),
                    PacketID::OpenConnectionRequest1 => todo!(),
                    PacketID::OpenConnectionReply1 => todo!(),
                    PacketID::OpenConnectionRequest2 => todo!(),
                    PacketID::OpenConnectionReply2 => todo!(),
                    PacketID::Unknown => todo!(),
                }
            }
        }
        
    }

    pub async fn set_motd(&mut self , mc_protocol_version : &str , mc_version : &str , guid : u64 ,  game_type : &str ,port : u16 ) {
        self.motd = format!("MCPE;{};{};{};0;{};{};Bedrock level;{};1;{};",SERVER_NAME, mc_protocol_version , mc_version , MAX_CONNECTION , guid , game_type,  port);
    }

    pub async fn get_motd(&self) -> String{
        self.motd.clone()
    }
}