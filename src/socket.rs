use std::{io::{Result} , net::{SocketAddr}, sync::{Arc}};
use tokio::{net::UdpSocket, sync::{Mutex, mpsc::channel}, time::{sleep, timeout}};
use rand;
use tokio::sync::mpsc::{Sender, Receiver};
use std::sync::atomic::{AtomicBool, Ordering};

use crate::{packet::*, utils::*, arq::*};

pub struct RaknetSocket{
    local_addr : SocketAddr,
    peer_addr : SocketAddr,
    s : Arc<UdpSocket>,
    user_data_sender : Arc<Mutex<Sender<Vec<u8>>>>,
    user_data_receiver : Receiver<Vec<u8>>,
    recvq : Arc<Mutex<RecvQ>>,
    sendq : Arc<Mutex<SendQ>>,
    connected : Arc<AtomicBool>,
    _guid : u64,
}

impl RaknetSocket {
    pub fn from(addr : &SocketAddr , s : &Arc<UdpSocket> ,receiver : Receiver<Vec<u8>> , mtu : u16) -> Self {

        let (user_data_sender , user_data_receiver) =  channel::<Vec<u8>>(100);

        let ret = RaknetSocket{
            peer_addr : addr.clone(),
            local_addr : s.local_addr().unwrap(),
            s: s.clone(),
            user_data_sender : Arc::new(Mutex::new(user_data_sender)),
            user_data_receiver : user_data_receiver,
            recvq : Arc::new(Mutex::new(RecvQ::new())),
            sendq : Arc::new(Mutex::new(SendQ::new(mtu))),
            connected : Arc::new(AtomicBool::new(true)),
            _guid : rand::random()
        };
        ret.start_receiver(receiver);
        ret.start_tick();
        ret
    }

    async fn handle (frame : &FrameSetPacket , peer_addr : &SocketAddr , local_addr : &SocketAddr , sendq : &Mutex<SendQ> , user_data_sender : &Mutex<Sender<Vec<u8>>>) -> bool {
        match PacketID::from(frame.data[0]) {
            PacketID::ConnectionRequest => {
                let packet = read_packet_connection_request(&frame.data.as_slice()).await.unwrap();
                
                let packet_reply = ConnectionRequestAccepted{
                    client_address: peer_addr.clone(),
                    system_index: 0,
                    request_timestamp: packet.time,
                    accepted_timestamp: cur_timestamp(),
                };

                let buf = write_packet_connection_request_accepted(&packet_reply).await.unwrap();
                sendq.lock().await.insert(Reliability::Reliable,&buf, cur_timestamp_millis());
            },
            PacketID::ConnectionRequestAccepted => {
                let packet = read_packet_connection_request_accepted(&frame.data.as_slice()).await.unwrap();
                
                let packet_reply = NewIncomingConnection{
                    server_address: local_addr.clone(),
                    request_timestamp: packet.request_timestamp,
                    accepted_timestamp: cur_timestamp(),
                };

                let buf = write_packet_new_incomming_connection(&packet_reply).await.unwrap();
                sendq.lock().await.insert(Reliability::Reliable ,&buf, cur_timestamp_millis());
            }
            PacketID::NewIncomingConnection => {
                let _packet = read_packet_new_incomming_connection(&frame.data.as_slice()).await.unwrap();
            }
            PacketID::ConnectedPing => {
                let packet = read_packet_connected_ping(&frame.data.as_slice()).await.unwrap();
                
                let packet_reply = ConnectedPong{
                    client_timestamp: packet.client_timestamp,
                    server_timestamp: cur_timestamp(),
                };

                let buf = write_packet_connected_pong(&packet_reply).await.unwrap();
                sendq.lock().await.insert(Reliability::Unreliable ,&buf, cur_timestamp_millis());
            }
            PacketID::ConnectedPong => {}
            PacketID::Disconnect => {
                return false;
            },
            _ => {
                match user_data_sender.lock().await.send(frame.data.clone()).await{
                    Ok(_) => {},
                    Err(_) => {
                        return false;
                    },
                };
            },
        }
        return true;
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
            mtu_size: RAKNET_CLIENT_MTU,
        };

        let buf = write_packet_connection_open_request_1(&packet).await.unwrap();

        s.send_to(&buf, addr).await.unwrap();

        let mut buf = [0u8 ; 2048];
        let (size ,src ) = s.recv_from(&mut buf).await.unwrap();

        if buf[0] != PacketID::OpenConnectionReply1.to_u8(){
            if buf[0] == PacketID::IncompatibleProtocolVersion.to_u8(){
                let packet = match read_packet_incompatible_protocol_version(&buf[..size]).await{
                    Ok(p) => p,
                    Err(e) => return Err(e),
                };

                return Err(std::io::Error::new(std::io::ErrorKind::Other, format!("server version : {}", packet.server_protocol)));
            }else{
                return Err(std::io::Error::new(std::io::ErrorKind::Other, "open connection reply1 packetid incorrect"));
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

        if buf[0] != PacketID::OpenConnectionReply2.to_u8(){
            return Err(std::io::Error::new(std::io::ErrorKind::Other, "open connection reply2 packetid incorrect"));
        }

        let _reply2 = match read_packet_connection_open_reply_2(&buf[..size]).await{
            Ok(p) => p,
            Err(e) => return Err(e),
        };

        let packet = ConnectionRequest{
            guid,
            time: cur_timestamp(),
            use_encryption: 0x00,
        };

        let buf = write_packet_connection_request(&packet).await.unwrap();

        let frame = FrameSetPacket::new(Reliability::Reliable, buf);

        s.send_to(&frame.serialize().await, addr).await.unwrap();

        let (user_data_sender , user_data_receiver) =  channel::<Vec<u8>>(100);

        let (sender , receiver) = channel::<Vec<u8>>(100);

        let s = Arc::new(s);

        let recv_s = s.clone();
        let connected = Arc::new(AtomicBool::new(true));
        let connected_s = connected.clone();
        tokio::spawn(async move {
            let mut buf = [0u8;2048];
            loop{
                if !connected_s.load(Ordering::Relaxed){
                    break;
                }
                let (size , _) = match match timeout(std::time::Duration::from_secs(10), recv_s.recv_from(&mut buf)).await{
                    Ok(p) => p,
                    Err(_) => continue
                }{
                    Ok(p) => p,
                    Err(_) => {
                        connected_s.store(false, Ordering::Relaxed);
                        break;
                    },
                };

                match sender.send(buf[..size].to_vec()).await{
                    Ok(_) => {},
                    Err(_) => {
                        connected_s.store(false, Ordering::Relaxed);
                        break;
                    },
                };
            }
            println!("finished recv_from");
        });

        let ret = RaknetSocket{
            peer_addr : addr.clone(),
            local_addr : s.local_addr().unwrap(),
            s: s,
            user_data_sender : Arc::new(Mutex::new(user_data_sender)),
            user_data_receiver : user_data_receiver,
            recvq : Arc::new(Mutex::new(RecvQ::new())),
            sendq : Arc::new(Mutex::new(SendQ::new(RAKNET_CLIENT_MTU))),
            connected : connected,
            _guid : guid
        };

        ret.start_receiver(receiver);
        ret.start_tick();
        Ok(ret)
    }

    fn start_receiver(&self , mut receiver : Receiver<Vec<u8>>) {
        let connected = self.connected.clone();
        let peer_addr = self.peer_addr.clone();
        let local_addr = self.local_addr.clone();
        let sendq = self.sendq.clone();
        let user_data_sender = self.user_data_sender.clone();
        let recvq = self.recvq.clone();
        tokio::spawn(async move {
            loop{
                let buf = match match timeout(std::time::Duration::from_secs(10) ,receiver.recv()).await{
                    Ok(p) => p,
                    Err(_) => {
                        if !connected.load(Ordering::Relaxed){
                            break;
                        }
                        continue;
                    }
                }{
                    Some(buf) => buf,
                    None => {
                        connected.store(false, Ordering::Relaxed);
                        break;
                    },
                };

                if PacketID::from(buf[0]) == PacketID::Disconnect{
                    connected.store(false, Ordering::Relaxed);
                    break;
                }

                if buf[0] == PacketID::ACK.to_u8(){
                    //handle ack
                    let mut sendq = sendq.lock().await;
                    let ack = read_packet_ack(&buf).await.unwrap();
                    if ack.single_sequence_number{
                        sendq.ack(ack.sequences.0);
                    } else{
                        for i in ack.sequences.0..ack.sequences.1+1{
                            sendq.ack(i);
                        }
                        
                    }
                }

                if buf[0] == PacketID::NACK.to_u8(){
                    //handle nack
                    let nack  = read_packet_nack(&buf).await.unwrap();

                    if nack.single_sequence_number {
                        sendq.lock().await.nack(nack.sequences.0 , cur_timestamp_millis());
                    } else {
                        let mut sendq = sendq.lock().await;
                        for i in nack.sequences.0..nack.sequences.1+1{
                            sendq.nack(i, cur_timestamp_millis());
                        }
                    }
                }

                // handle packet in here
                if buf[0] >= PacketID::FrameSetPacketBegin.to_u8() && 
                   buf[0] <= PacketID::FrameSetPacketEnd.to_u8() {

                    let frames = FrameVec::new(buf.clone()).await;

                    for frame in frames.frames{
                        
                        let mut recvq = recvq.lock().await;
                        recvq.insert(frame);

                        for f in recvq.flush(){
                            if !RaknetSocket::handle(&f , &peer_addr ,&local_addr, &sendq, &user_data_sender ).await{
                                connected.store(false, Ordering::Relaxed);
                                return;
                            };
                        }
                    }
                }
            }

            println!("finished receiver");
        });
    }

    fn start_tick(&self) {
        let connected = self.connected.clone();
        let s = self.s.clone();
        let peer_addr = self.peer_addr.clone();
        let sendq = self.sendq.clone();
        let recvq = self.recvq.clone();
        tokio::spawn(async move {
            loop{
                sleep(std::time::Duration::from_millis(50)).await;

                if !connected.load(Ordering::Relaxed){
                    break;
                }

                // flush ack
                let mut recvq = recvq.lock().await;
                let acks = recvq.get_ack();

                for ack in acks {

                    let single_sequence_number = ack.1 == ack.0;

                    let packet = ACK{
                        // catch minecraft 1.18.2 client packet , the parameter always 1
                        record_count: 1,
                        single_sequence_number: single_sequence_number,
                        sequences: ack,
                    };

                    let buf = write_packet_ack(&packet).await.unwrap();
                    s.send_to(&buf, peer_addr).await.unwrap();
                }
                
                //flush sendq
                let mut sendq = sendq.lock().await;
                sendq.tick(cur_timestamp_millis());
                for f in sendq.flush(){
                    s.send_to(f.serialize().await.as_slice(), peer_addr).await.unwrap();
                }
            }
            println!("finished tick");
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
        match self.s.send_to(&[PacketID::Disconnect.to_u8()], self.peer_addr).await{
            Ok(_) => {
                self.connected.store(false, Ordering::Relaxed);
                Ok(())
            },
            Err(_) => {
                self.connected.store(false, Ordering::Relaxed);
                Err(std::io::Error::new(std::io::ErrorKind::Other , "send disconnect message faild , but connection still closed"))
            },
        }
    }

    pub async fn send(&mut self , buf : &[u8] , r : Reliability) ->Result<()> {

        self.sendq.lock().await.insert(r , buf, cur_timestamp_millis());
        Ok(())
    }

    pub async fn recv(&mut self) -> Result<Vec<u8>> {

        if !self.connected.load(Ordering::Relaxed){
            return Err(std::io::Error::new(std::io::ErrorKind::Other, "conntection closed"));
        }

        loop{
            match match timeout(std::time::Duration::from_secs(10), self.user_data_receiver.recv()).await{
                Ok(p) => p,
                Err(_) => {
                    if !self.connected.load(Ordering::Relaxed){
                        return Err(std::io::Error::new(std::io::ErrorKind::Other, "conntection closed"));
                    }
                    continue;
                },
            }{
                Some(p) => return Ok(p),
                None => {
                    return Err(std::io::Error::new(std::io::ErrorKind::Other, "recv packet faild , maybe is conntection closed"));
                },
            }
        }

    }

    pub fn peer_addr(&self) -> Result<SocketAddr>{
        Ok(self.peer_addr)
    }

    pub fn local_addr(&self) -> Result<SocketAddr>{
        Ok(self.local_addr)
    }
}