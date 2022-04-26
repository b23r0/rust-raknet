use std::{net::{SocketAddr}, sync::{Arc, atomic::{AtomicU8, AtomicI64}}};
use rand::Rng;
use tokio::{net::UdpSocket, sync::{Mutex, mpsc::channel, Notify}, time::{sleep, timeout}};

use tokio::sync::mpsc::{Sender, Receiver};
use std::sync::atomic::{AtomicBool, Ordering};
use crate::{error::{Result, RaknetError}, raknet_log_error};

use crate::{packet::*, utils::*, arq::*, raknet_log_debug};

/// Raknet socket wrapper with local and remote.
pub struct RaknetSocket{
    local_addr : SocketAddr,
    peer_addr : SocketAddr,
    s : Arc<UdpSocket>,
    user_data_receiver : Receiver<Vec<u8>>,
    recvq : Arc<Mutex<RecvQ>>,
    sendq : Arc<Mutex<SendQ>>,
    connected : Arc<AtomicBool>,
    last_heartbeat_time : Arc<AtomicI64>,
    enable_loss : Arc<AtomicBool>,
    loss_rate : Arc<AtomicU8>,
    incomming_notify : Arc<Notify>,
}

impl RaknetSocket {
    /// Create a Raknet Socket from a UDP socket with an established Raknet connection
    /// 
    /// This method is used for RaknetListener, users of the library should not care about it.
    pub fn from(addr : &SocketAddr , s : &Arc<UdpSocket> ,receiver : Receiver<Vec<u8>> , mtu : u16 , collecter : Arc<Mutex<Sender<SocketAddr>>>) -> Self {

        let (user_data_sender , user_data_receiver) =  channel::<Vec<u8>>(100);

        let ret = RaknetSocket{
            peer_addr : *addr,
            local_addr : s.local_addr().unwrap(),
            s: s.clone(),
            user_data_receiver,
            recvq : Arc::new(Mutex::new(RecvQ::new())),
            sendq : Arc::new(Mutex::new(SendQ::new(mtu))),
            connected : Arc::new(AtomicBool::new(true)),
            last_heartbeat_time : Arc::new(AtomicI64::new(cur_timestamp_millis())),
            enable_loss : Arc::new(AtomicBool::new(false)),
            loss_rate : Arc::new(AtomicU8::new(0)),
            incomming_notify : Arc::new(Notify::new()),
        };
        ret.start_receiver(receiver , user_data_sender);
        ret.start_tick(Some(collecter));
        ret
    }

    async fn handle (frame : &FrameSetPacket , peer_addr : &SocketAddr , local_addr : &SocketAddr , sendq : &Mutex<SendQ> , user_data_sender : &Sender<Vec<u8>> , incomming_notify : &Notify) -> Result<bool> {
        match PacketID::from(frame.data[0])? {
            PacketID::ConnectionRequest => {
                let packet = read_packet_connection_request(frame.data.as_slice()).await?;
                
                let packet_reply = ConnectionRequestAccepted{
                    client_address: *peer_addr,
                    system_index: 0,
                    request_timestamp: packet.time,
                    accepted_timestamp: cur_timestamp_millis(),
                };

                let buf = write_packet_connection_request_accepted(&packet_reply).await?;
                sendq.lock().await.insert(Reliability::ReliableOrdered,&buf)?;
            },
            PacketID::ConnectionRequestAccepted => {
                let packet = read_packet_connection_request_accepted(frame.data.as_slice()).await?;
                
                let packet_reply = NewIncomingConnection{
                    server_address: *local_addr,
                    request_timestamp: packet.request_timestamp,
                    accepted_timestamp: cur_timestamp_millis(),
                };

                let mut sendq = sendq.lock().await;

                let buf = write_packet_new_incomming_connection(&packet_reply).await?;
                sendq.insert(Reliability::ReliableOrdered ,&buf)?;

                let ping = ConnectedPing{
                    client_timestamp: cur_timestamp_millis(),
                };

                //i dont know why incomming packet after always follow a connected ping packet in minecraft bedrock 1.18.12.
                let buf = write_packet_connected_ping(&ping).await?;
                sendq.insert(Reliability::Unreliable ,&buf)?;
                raknet_log_debug!("incomming notified");
                incomming_notify.notify_one();
            }
            PacketID::NewIncomingConnection => {
                let _packet = read_packet_new_incomming_connection(frame.data.as_slice()).await?;
            }
            PacketID::ConnectedPing => {
                let packet = read_packet_connected_ping(frame.data.as_slice()).await?;
                
                let packet_reply = ConnectedPong{
                    client_timestamp: packet.client_timestamp,
                    server_timestamp: cur_timestamp_millis(),
                };

                let buf = write_packet_connected_pong(&packet_reply).await?;
                sendq.lock().await.insert(Reliability::Unreliable ,&buf)?;
            }
            PacketID::ConnectedPong => {}
            PacketID::Disconnect => {
                return Ok(false);
            },
            _ => {
                match user_data_sender.send(frame.data.clone()).await{
                    Ok(_) => {},
                    Err(_) => {
                        return Ok(false);
                    },
                };
            },
        }
        Ok(true)
    }
    
    async fn sendto(s : &UdpSocket , buf : &[u8] , target : &SocketAddr , enable_loss : &AtomicBool , loss_rate : &AtomicU8) -> tokio::io::Result<usize>{
        if enable_loss.load(Ordering::Relaxed){
            let mut rng = rand::thread_rng();
            let i: u8 = rng.gen_range(0..11);
            if i > loss_rate.load(Ordering::Relaxed) {
                raknet_log_debug!("loss packet");
                return Ok(0);
            }
        }
        match s.send_to(buf, target).await{
            Ok(p) => Ok(p),
            Err(e) => {
                raknet_log_error!("udp socket send_to error : {}" ,e);
                Err(e)
            },
        }
    }
    
    /// Connect to a Raknet server and return a Raknet socket
    /// 
    /// # Example
    /// ```ignore
    /// let mut socket = RaknetSocket::connect("127.0.0.1:19132".parse().unwrap()).await.unwrap();
    /// socket.send(&[0xfe], Reliability::ReliableOrdered).await.unwrap();
    /// let buf = socket.recv().await.unwrap();
    /// if buf[0] == 0xfe{
    ///    //do something
    /// }
    /// socket.close().await.unwarp(); // you need to manually close raknet connection
    /// ```
    pub async fn connect(addr : &SocketAddr) -> Result<Self>{

        let guid : u64 = rand::random();

        let s = match UdpSocket::bind("0.0.0.0:0").await{
            Ok(p) => p,
            Err(_) => return Err(RaknetError::BindAdreesError),
        };

        let packet = OpenConnectionRequest1{
            magic: true,
            protocol_version: RAKNET_PROTOCOL_VERSION,
            mtu_size: RAKNET_CLIENT_MTU,
        };

        let buf = write_packet_connection_open_request_1(&packet).await.unwrap();

        let remote_addr : SocketAddr;
        let reply1_size : usize;

        let mut reply1_buf =  [0u8 ; 2048];

        loop{
            s.send_to(&buf, addr).await.unwrap();
            let (size ,src ) = match match timeout(std::time::Duration::from_secs(2),s.recv_from(&mut reply1_buf)).await{
                Ok(p) => p,
                Err(_) =>{
                    raknet_log_debug!("wait reply1 timeout");
                    continue;
                }
            }{
                Ok(p) => p,
                Err(e) => {
                    raknet_log_error!("recvfrom error : {}" , e);
                    continue;
                }
            };

            remote_addr = src;
            reply1_size = size;

            if reply1_buf[0] != PacketID::OpenConnectionReply1.to_u8(){
                if reply1_buf[0] == PacketID::IncompatibleProtocolVersion.to_u8(){
                    let _packet = match read_packet_incompatible_protocol_version(&buf[..size]).await{
                        Ok(p) => p,
                        Err(_) => return Err(RaknetError::NotSupportVersion),
                    };
    
                    return Err(RaknetError::NotSupportVersion);
                }else{
                    raknet_log_debug!("incorrect reply1");
                    return Err(RaknetError::IncorrectReply);
                }
            }

            break;
        }


        let reply1 = match read_packet_connection_open_reply_1(&reply1_buf[..reply1_size]).await{
            Ok(p) => p,
            Err(_) => return Err(RaknetError::PacketParseError),
        };

        let packet = OpenConnectionRequest2{
            magic: true,
            address: remote_addr,
            mtu: reply1.mtu_size,
            guid,
        };

        let buf = write_packet_connection_open_request_2(&packet).await.unwrap();

        loop{
            s.send_to(&buf, addr).await.unwrap();

            let mut buf = [0u8 ; 2048];
            let (size ,_ ) = match match timeout(std::time::Duration::from_secs(2) ,s.recv_from(&mut buf)).await{
                Ok(p) => p,
                Err(_) => {
                    raknet_log_debug!("wait reply2 timeout");
                    continue;
                },
            }{
                Ok(p) => p,
                Err(e) => {
                    raknet_log_error!("recvfrom error : {}" , e);
                    continue;
                }
            };

            if buf[0] != PacketID::OpenConnectionReply2.to_u8(){
                raknet_log_debug!("incorrect reply2");
                return Err(RaknetError::IncorrectReply);
            }
    
            let _reply2 = match read_packet_connection_open_reply_2(&buf[..size]).await{
                Ok(p) => p,
                Err(_) => return Err(RaknetError::PacketParseError),
            };

            break;
        }

        let sendq = Arc::new(Mutex::new(SendQ::new(reply1.mtu_size)));

        let packet = ConnectionRequest{
            guid,
            time: cur_timestamp_millis(),
            use_encryption: 0x00,
        };

        let buf = write_packet_connection_request(&packet).await.unwrap();

        let mut sendq1 = sendq.lock().await;
        sendq1.insert(Reliability::ReliableOrdered, &buf)?;
        std::mem::drop(sendq1);

        let (user_data_sender , user_data_receiver) =  channel::<Vec<u8>>(100);

        let (sender , receiver) = channel::<Vec<u8>>(100);

        let s = Arc::new(s);

        let recv_s = s.clone();
        let connected = Arc::new(AtomicBool::new(true));
        let connected_s = connected.clone();
        let peer_addr = *addr;
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
                    Err(e) => {
                        #[cfg(target_family = "windows")]
                        if e.raw_os_error().unwrap() == 10040{
                            // https://docs.microsoft.com/zh-CN/troubleshoot/windows-server/networking/wsaemsgsize-error-10040-in-winsock-2
                            raknet_log_debug!("recv_from error : {}" , e.raw_os_error().unwrap());
                            continue;
                        }
                        raknet_log_debug!("recv_from error : {}" , e);
                        connected_s.store(false, Ordering::Relaxed);
                        break;
                    },
                };

                match sender.send(buf[..size].to_vec()).await{
                    Ok(_) => {},
                    Err(e) => {
                        raknet_log_debug!("channel send error : {}" , e);
                        connected_s.store(false, Ordering::Relaxed);
                        break;
                    },
                };
            }
            raknet_log_debug!("{} , recv_from finished" , peer_addr );
        });

        let ret = RaknetSocket{
            peer_addr : *addr,
            local_addr : s.local_addr().unwrap(),
            s,
            user_data_receiver,
            recvq : Arc::new(Mutex::new(RecvQ::new())),
            sendq,
            connected,
            last_heartbeat_time : Arc::new(AtomicI64::new(cur_timestamp_millis())),
            enable_loss : Arc::new(AtomicBool::new(false)),
            loss_rate : Arc::new(AtomicU8::new(0)),
            incomming_notify : Arc::new(Notify::new()),
        };

        ret.start_receiver(receiver , user_data_sender);
        ret.start_tick(None);

        raknet_log_debug!("wait incomming notify");
        ret.incomming_notify.notified().await;
        
        Ok(ret)
    }

    fn start_receiver(&self , mut receiver : Receiver<Vec<u8>> , user_data_sender : Sender<Vec<u8>>) {
        let connected = self.connected.clone();
        let peer_addr = self.peer_addr;
        let local_addr = self.local_addr;
        let sendq = self.sendq.clone();
        let recvq = self.recvq.clone();
        let last_heartbeat_time = self.last_heartbeat_time.clone();
        let incomming_notify = self.incomming_notify.clone();
        let s = self.s.clone();
        let enable_loss = self.enable_loss.clone();
        let loss_rate = self.loss_rate.clone();
        tokio::spawn(async move {
            loop{
                if !connected.load(Ordering::Relaxed){
                    break;
                }

                let buf = match receiver.recv().await{
                    Some(buf) => buf,
                    None => {
                        raknet_log_debug!("channel receiver finished");
                        connected.store(false, Ordering::Relaxed);
                        break;
                    },
                };

                last_heartbeat_time.store(cur_timestamp_millis(), Ordering::Relaxed);

                if PacketID::from(buf[0]).unwrap() == PacketID::Disconnect{
                    connected.store(false, Ordering::Relaxed);
                    break;
                }

                if buf[0] == PacketID::ACK.to_u8(){
                    //handle ack
                    let mut sendq = sendq.lock().await;
                    let ack = read_packet_ack(&buf).await.unwrap();
                    for i in 0..ack.record_count{
                        if ack.sequences[i as usize].0 == ack.sequences[i as usize].1{
                            sendq.ack(ack.sequences[i as usize].0 , cur_timestamp_millis());
                        } else{
                            for i in ack.sequences[i as usize].0..ack.sequences[i as usize].1+1{
                                sendq.ack(i , cur_timestamp_millis());
                            }
                            
                        }
                    }
                    continue;
                }

                if buf[0] == PacketID::NACK.to_u8(){
                    //handle nack
                    let nack  = read_packet_nack(&buf).await.unwrap();

                    let mut sendq = sendq.lock().await;

                    for i in 0..nack.record_count{
                        if nack.sequences[i as usize].0 == nack.sequences[i as usize].1 {
                            sendq.nack(nack.sequences[i as usize].0 , cur_timestamp_millis());
                        } else {
                            for i in nack.sequences[i as usize].0..nack.sequences[i as usize].1+1{
                                sendq.nack(i, cur_timestamp_millis());
                            }
                        }
                    }
                    continue;
                }

                // handle packet in here
                if buf[0] >= PacketID::FrameSetPacketBegin.to_u8() && 
                   buf[0] <= PacketID::FrameSetPacketEnd.to_u8() {

                    let frames = FrameVec::new(buf.clone()).await.unwrap();

                    let mut recvq = recvq.lock().await;
                    let mut is_break = false;
                    for frame in frames.frames{
                        recvq.insert(frame).unwrap();

                        for f in recvq.flush(&peer_addr){
                            if !RaknetSocket::handle(&f , &peer_addr ,&local_addr, &sendq, &user_data_sender , &incomming_notify).await.unwrap(){
                                raknet_log_error!("handle faild");
                                connected.store(false, Ordering::Relaxed);
                                is_break = true;
                                break;
                            };
                        }

                        if is_break {
                            break;
                        }
                    }

                    //flush ack
                    let acks = recvq.get_ack();

                    if !acks.is_empty() {
    
                        let packet = ACK{
                            record_count: acks.len() as u16,
                            sequences: acks,
                        };
    
                        let buf = write_packet_ack(&packet).await.unwrap();
                        RaknetSocket::sendto(&s , &buf, &peer_addr , &enable_loss  , &loss_rate).await.unwrap();
                    }
                } else {
                    raknet_log_debug!("unknown packetid : {}", buf[0]);
                }
            }

            raknet_log_debug!("{} , receiver finished" , peer_addr);
        });
    }

    fn start_tick(&self , collecter : Option<Arc<Mutex<Sender<SocketAddr>>>>) {
        let connected = self.connected.clone();
        let s = self.s.clone();
        let peer_addr = self.peer_addr;
        let sendq = self.sendq.clone();
        let recvq = self.recvq.clone();
        let mut last_monitor_tick = cur_timestamp_millis();
        let enable_loss = self.enable_loss.clone();
        let loss_rate = self.loss_rate.clone();
        let last_heartbeat_time = self.last_heartbeat_time.clone();
        tokio::spawn(async move {
            loop{
                sleep(std::time::Duration::from_millis(SendQ::DEFAULT_TIMEOUT_MILLS as u64)).await;

                // flush nack
                let mut recvq = recvq.lock().await;
                let nacks = recvq.get_nack();
                if !nacks.is_empty(){
                    let nack = NACK{
                        record_count: nacks.len() as u16,
                        sequences: nacks,
                    };

                    let buf = write_packet_nack(&nack).await.unwrap();
                    match RaknetSocket::sendto(&s , &buf, &peer_addr , &enable_loss , &loss_rate).await{
                        Ok(_) => {},
                        Err(_) => {
                            break;
                        },
                    };
                }
                
                //flush sendq
                let mut sendq = sendq.lock().await;
                for f in sendq.flush(cur_timestamp_millis(), &peer_addr){
                    let data = f.serialize().await.unwrap();
                    RaknetSocket::sendto(&s , &data, &peer_addr , &enable_loss  , &loss_rate).await.unwrap();
                }

                //monitor log
                if cur_timestamp_millis() - last_monitor_tick > 10000{
                    raknet_log_debug!("peer addr : {} , sendq size : {} , sentq size : {} , rto : {} , recvq size : {} ,  recvq fragment size : {} , ordered queue size : {} - {:?}" , 
                        peer_addr,
                        sendq.get_reliable_queue_size(),
                        sendq.get_sent_queue_size(),
                        sendq.get_rto(),
                        recvq.get_size(),
                        recvq.get_fragment_queue_size(),
                        recvq.get_ordered_packet(),
                        recvq.get_ordered_keys()
                    );
                    last_monitor_tick = cur_timestamp_millis();
                }

                // if exceed 60s not received any packet will close connection.
                if cur_timestamp_millis() - last_heartbeat_time.load(Ordering::Relaxed) > RECEIVE_TIMEOUT{
                    raknet_log_debug!("recv timeout");
                    connected.store(false, Ordering::Relaxed);
                    break;
                }

                if !connected.load(Ordering::Relaxed){
                    break;
                }
                
            }

            match collecter{
                Some(p) => {
                    p.lock().await.send(peer_addr).await.unwrap();
                },
                None => {},
            }
            raknet_log_debug!("{} , ticker finished" , peer_addr);
        });
    }

    /// Close Raknet Socket
    /// 
    /// The Raknet Socket needs to be closed manually, and if no valid data packets are received for more than 1 minute, the Raknet connection will be closed automatically. 
    /// This method can be called repeatedly.
    /// 
    /// # Example
    /// ```ignore
    /// let mut socket = RaknetSocket::connect("127.0.0.1:19132".parse().unwrap()).await.unwrap();
    /// socket.close().await.unwarp();
    /// ```
    pub async fn close(&mut self) -> Result<()>{

        if self.connected.load(Ordering::Relaxed){
            self.sendq.lock().await.insert(Reliability::Reliable, &[PacketID::Disconnect.to_u8()])?;
            self.connected.store(false, Ordering::Relaxed);
        }
        Ok(())
    }

    /// Unconnected ping a Raknet Server and return latency
    /// 
    /// # Example
    /// ```ignore
    /// let latency = socket::RaknetSocket::ping("127.0.0.1:19132".parse().unwrap()).await.unwrap();
    /// assert!((0..10).contains(&latency));
    /// ```
    pub async fn ping(addr : &SocketAddr) -> Result<i64> {
        let packet = PacketUnconnectedPing{
            time: cur_timestamp_millis(),
            magic: true,
            guid: rand::random(),
        };

        let s = match UdpSocket::bind("0.0.0.0:0").await{
            Ok(p) => p,
            Err(_) => return Err(RaknetError::BindAdreesError),
        };

        let buf = write_packet_ping(&packet).await?;

        s.send_to(buf.as_slice(), addr).await.unwrap();

        let mut buf = [0u8 ; 1024];
        match s.recv_from(&mut buf).await{
            Ok(p) => p,
            Err(_) => return Err(RaknetError::RecvFromError),
        };

        let pong = match read_packet_pong(&buf).await{
            Ok(p) => p,
            Err(_) => return Err(RaknetError::PacketParseError),
        };

        Ok(pong.time - packet.time)
    }

    /// Send a packet
    /// 
    /// packet must be `0xfe` as the first byte, using other values of bytes may cause unexpected errors.
    /// 
    /// Except Reliability::ReliableOrdered, all other reliability packets must be less than MTU - 60 (default 1340 bytes), otherwise RaknetError::PacketSizeExceedMTU will be returned
    /// 
    /// # Example
    /// ```ignore
    /// let mut socket = RaknetSocket::connect("127.0.0.1:19132".parse().unwrap()).await.unwrap();
    /// socket.send(&[0xfe], Reliability::ReliableOrdered).await.unwrap();
    /// ```
    pub async fn send(&mut self , buf : &[u8] , r : Reliability) ->Result<()> {

        if !self.connected.load(Ordering::Relaxed){
            return Err(RaknetError::ConnectionClosed);
        }

        //flush sendq
        let mut sendq = self.sendq.lock().await;
        sendq.insert(r , buf)?;
        for f in sendq.flush(cur_timestamp_millis(), &self.peer_addr){
            let data = f.serialize().await.unwrap();
            RaknetSocket::sendto(&self.s , &data, &self.peer_addr , &self.enable_loss , &self.loss_rate).await.unwrap();
        }
        Ok(())
    }

    /// Recv a packet
    /// 
    /// # Example
    /// ```ignore
    /// let mut socket = RaknetSocket::connect("127.0.0.1:19132".parse().unwrap()).await.unwrap();
    /// let buf = socket.recv().await.unwrap();
    /// if buf[0] == 0xfe{
    ///    //do something
    /// }
    /// ```
    pub async fn recv(&mut self) -> Result<Vec<u8>> {

        if !self.connected.load(Ordering::Relaxed){
            return Err(RaknetError::ConnectionClosed);
        }

        match self.user_data_receiver.recv().await{
            Some(p) => Ok(p),
            None => {
                if !self.connected.load(Ordering::Relaxed){
                    return Err(RaknetError::ConnectionClosed);
                }
                Err(RaknetError::RecvFromError)
            },
        }

    }

    /// Returns the socket address of the remote peer of this Raknet connection.
    /// 
    /// # Example
    /// ```ignore
    /// let socket = RaknetSocket::connect("127.0.0.1:19132".parse().unwrap()).await.unwrap();
    /// assert_eq!(socket.peer_addr().unwrap(), SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::new(127, 0, 0, 1), 19132)));
    /// ```
    pub fn peer_addr(&self) -> Result<SocketAddr>{
        Ok(self.peer_addr)
    }

    /// Returns the socket address of the local half of this Raknet connection.
    /// 
    /// # Example
    /// ```ignore
    /// let mut socket = RaknetSocket::connect("127.0.0.1:19132".parse().unwrap()).await.unwrap();
    /// assert_eq!(socket.local_addr().unwrap().ip(), IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)));
    /// ```
    pub fn local_addr(&self) -> Result<SocketAddr>{
        Ok(self.local_addr)
    }

    /// Set the packet loss rate and use it for testing
    /// 
    /// The `stage` parameter ranges from 0 to 10, indicating a packet loss rate of 0% to 100%.
    /// # Example
    /// ```ignore
    /// let mut socket = RaknetSocket::connect("127.0.0.1:19132".parse().unwrap()).await.unwrap();
    /// // set 20% loss packet rate.
    /// socket.set_loss_rate(8);
    /// ```
    pub fn set_loss_rate(&mut self ,stage : u8){
        self.enable_loss.store(true, Ordering::Relaxed);
        self.loss_rate.store(stage, Ordering::Relaxed);

    }
}