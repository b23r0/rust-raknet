use rand::Rng;
use std::{
    net::SocketAddr,
    sync::{
        atomic::{AtomicI64, AtomicU8},
        Arc,
    },
};
use tokio::{
    net::UdpSocket,
    sync::{mpsc::channel, Mutex, Notify, RwLock},
    time::{sleep, timeout},
};

use crate::{
    error::{RaknetError, Result},
    raknet_log_error, raknet_log_info,
};
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::mpsc::{Receiver, Sender};

use crate::{arq::*, packet::*, raknet_log_debug, utils::*};

/// Raknet socket wrapper with local and remote.
pub struct RaknetSocket {
    local_addr: SocketAddr,
    peer_addr: SocketAddr,
    user_data_receiver: Arc<Mutex<Receiver<Vec<u8>>>>,
    recvq: Arc<Mutex<RecvQ>>,
    sendq: Arc<RwLock<SendQ>>,
    close_notifier: Arc<tokio::sync::Semaphore>,
    last_heartbeat_time: Arc<AtomicI64>,
    enable_loss: Arc<AtomicBool>,
    loss_rate: Arc<AtomicU8>,
    incomming_notifier: Arc<Notify>,
    sender: Sender<(Vec<u8>, SocketAddr, bool, u8)>,
    drop_notifier: Arc<Notify>,
    raknet_version: u8,
}

impl RaknetSocket {
    /// Create a Raknet Socket from a UDP socket with an established Raknet connection
    ///
    /// This method is used for RaknetListener, users of the library should not care about it.
    pub async fn from(
        addr: &SocketAddr,
        s: &Arc<UdpSocket>,
        receiver: Receiver<Vec<u8>>,
        mtu: u16,
        collecter: Arc<Mutex<Sender<SocketAddr>>>,
        raknet_version: u8,
    ) -> Self {
        let (user_data_sender, user_data_receiver) = channel::<Vec<u8>>(100);
        let (sender_sender, sender_receiver) = channel::<(Vec<u8>, SocketAddr, bool, u8)>(10);

        let ret = RaknetSocket {
            peer_addr: *addr,
            local_addr: s.local_addr().unwrap(),
            user_data_receiver: Arc::new(Mutex::new(user_data_receiver)),
            recvq: Arc::new(Mutex::new(RecvQ::new())),
            sendq: Arc::new(RwLock::new(SendQ::new(mtu))),
            close_notifier: Arc::new(tokio::sync::Semaphore::new(0)),
            last_heartbeat_time: Arc::new(AtomicI64::new(cur_timestamp_millis())),
            enable_loss: Arc::new(AtomicBool::new(false)),
            loss_rate: Arc::new(AtomicU8::new(0)),
            incomming_notifier: Arc::new(Notify::new()),
            sender: sender_sender,
            drop_notifier: Arc::new(Notify::new()),
            raknet_version,
        };
        ret.start_receiver(s, receiver, user_data_sender);
        ret.start_tick(s, Some(collecter));
        ret.start_sender(s, sender_receiver);
        ret.drop_watcher().await;
        ret
    }

    async fn handle(
        frame: &FrameSetPacket,
        peer_addr: &SocketAddr,
        local_addr: &SocketAddr,
        sendq: &RwLock<SendQ>,
        user_data_sender: &Sender<Vec<u8>>,
        incomming_notify: &Notify,
    ) -> Result<bool> {
        match PacketID::from(frame.data[0])? {
            PacketID::ConnectionRequest => {
                let packet = read_packet_connection_request(frame.data.as_slice())?;

                let packet_reply = ConnectionRequestAccepted {
                    client_address: *peer_addr,
                    system_index: 0,
                    request_timestamp: packet.time,
                    accepted_timestamp: cur_timestamp_millis(),
                };

                let buf = write_packet_connection_request_accepted(&packet_reply)?;
                sendq
                    .write()
                    .await
                    .insert(Reliability::ReliableOrdered, &buf)?;
            }
            PacketID::ConnectionRequestAccepted => {
                let packet = read_packet_connection_request_accepted(frame.data.as_slice())?;

                let packet_reply = NewIncomingConnection {
                    server_address: *local_addr,
                    request_timestamp: packet.request_timestamp,
                    accepted_timestamp: cur_timestamp_millis(),
                };

                let mut sendq = sendq.write().await;

                let buf = write_packet_new_incomming_connection(&packet_reply)?;
                sendq.insert(Reliability::ReliableOrdered, &buf)?;

                let ping = ConnectedPing {
                    client_timestamp: cur_timestamp_millis(),
                };

                //i dont know why incomming packet after always follow a connected ping packet in minecraft bedrock 1.18.12.
                let buf = write_packet_connected_ping(&ping)?;
                sendq.insert(Reliability::Unreliable, &buf)?;
                raknet_log_debug!("incomming notified");
                incomming_notify.notify_one();
            }
            PacketID::NewIncomingConnection => {
                let _packet = read_packet_new_incomming_connection(frame.data.as_slice())?;
            }
            PacketID::ConnectedPing => {
                let packet = read_packet_connected_ping(frame.data.as_slice())?;

                let packet_reply = ConnectedPong {
                    client_timestamp: packet.client_timestamp,
                    server_timestamp: cur_timestamp_millis(),
                };

                let buf = write_packet_connected_pong(&packet_reply)?;
                sendq.write().await.insert(Reliability::Unreliable, &buf)?;
            }
            PacketID::ConnectedPong => {}
            PacketID::Disconnect => {
                return Ok(false);
            }
            _ => {
                match user_data_sender.send(frame.data.clone()).await {
                    Ok(_) => {}
                    Err(_) => {
                        return Ok(false);
                    }
                };
            }
        }
        Ok(true)
    }

    async fn sendto(
        s: &UdpSocket,
        buf: &[u8],
        target: &SocketAddr,
        enable_loss: bool,
        loss_rate: u8,
    ) -> tokio::io::Result<usize> {
        if enable_loss {
            let mut rng = rand::thread_rng();
            let i: u8 = rng.gen_range(0..11);
            if i > loss_rate {
                raknet_log_debug!("loss packet");
                return Ok(0);
            }
        }
        match s.send_to(buf, target).await {
            Ok(p) => Ok(p),
            Err(e) => {
                raknet_log_error!("udp socket send_to error : {}", e);
                Ok(0)
            }
        }
    }

    /// Connect to a Raknet server and return a Raknet socket
    ///
    /// # Example
    /// ```ignore
    /// let socket = RaknetSocket::connect("127.0.0.1:19132".parse().unwrap()).await.unwrap();
    /// socket.send(&[0xfe], Reliability::ReliableOrdered).await.unwrap();
    /// let buf = socket.recv().await.unwrap();
    /// if buf[0] == 0xfe{
    ///    //do something
    /// }
    /// ```

    pub async fn connect(addr: &SocketAddr) -> Result<Self> {
        Self::connect_with_version(addr, RAKNET_PROTOCOL_VERSION).await
    }

    pub async fn connect_with_version(addr: &SocketAddr, raknet_version: u8) -> Result<Self> {
        let guid: u64 = rand::random();

        let s = match UdpSocket::bind("0.0.0.0:0").await {
            Ok(p) => p,
            Err(_) => return Err(RaknetError::BindAddressError),
        };

        let packet = OpenConnectionRequest1 {
            magic: true,
            protocol_version: raknet_version,
            mtu_size: RAKNET_CLIENT_MTU,
        };

        let buf = write_packet_connection_open_request_1(&packet).unwrap();

        let mut remote_addr: SocketAddr;
        let mut reply1_size: usize;

        let mut reply1_buf = [0u8; 2048];

        loop {
            match s.send_to(&buf, addr).await {
                Ok(p) => p,
                Err(e) => {
                    raknet_log_error!("udp socket sendto error {}", e);
                    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                    continue;
                }
            };
            let (size, src) = match match timeout(
                std::time::Duration::from_secs(2),
                s.recv_from(&mut reply1_buf),
            )
            .await
            {
                Ok(p) => p,
                Err(_) => {
                    raknet_log_debug!("wait reply1 timeout");
                    continue;
                }
            } {
                Ok(p) => p,
                Err(e) => {
                    raknet_log_error!("recvfrom error : {}", e);
                    continue;
                }
            };

            remote_addr = src;
            reply1_size = size;

            if reply1_buf[0] != PacketID::OpenConnectionReply1.to_u8() {
                if reply1_buf[0] == PacketID::IncompatibleProtocolVersion.to_u8() {
                    let _packet = match read_packet_incompatible_protocol_version(&buf[..size]) {
                        Ok(p) => p,
                        Err(_) => return Err(RaknetError::NotSupportVersion),
                    };

                    return Err(RaknetError::NotSupportVersion);
                } else {
                    raknet_log_debug!("incorrect reply1");
                    continue;
                }
            }

            break;
        }

        let reply1 = match read_packet_connection_open_reply_1(&reply1_buf[..reply1_size]) {
            Ok(p) => p,
            Err(_) => return Err(RaknetError::PacketParseError),
        };

        let packet = OpenConnectionRequest2 {
            magic: true,
            address: remote_addr,
            mtu: reply1.mtu_size,
            guid,
        };

        let buf = write_packet_connection_open_request_2(&packet).unwrap();

        loop {
            match s.send_to(&buf, addr).await {
                Ok(_) => {}
                Err(e) => {
                    raknet_log_error!("udp socket sendto error {}", e);
                    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                    continue;
                }
            };

            let mut buf = [0u8; 2048];
            let (size, _) =
                match match timeout(std::time::Duration::from_secs(2), s.recv_from(&mut buf)).await
                {
                    Ok(p) => p,
                    Err(_) => {
                        raknet_log_debug!("wait reply2 timeout");
                        continue;
                    }
                } {
                    Ok(p) => p,
                    Err(e) => {
                        raknet_log_error!("recvfrom error : {}", e);
                        continue;
                    }
                };

            if buf[0] == PacketID::OpenConnectionReply1.to_u8() {
                raknet_log_debug!("repeat receive reply1");
                continue;
            }

            if buf[0] != PacketID::OpenConnectionReply2.to_u8() {
                raknet_log_debug!("incorrect reply2");
                continue;
            }

            let _reply2 = match read_packet_connection_open_reply_2(&buf[..size]) {
                Ok(p) => p,
                Err(_) => return Err(RaknetError::PacketParseError),
            };

            break;
        }

        let sendq = Arc::new(RwLock::new(SendQ::new(reply1.mtu_size)));

        let packet = ConnectionRequest {
            guid,
            time: cur_timestamp_millis(),
            use_encryption: 0x00,
        };

        let buf = write_packet_connection_request(&packet).unwrap();

        let mut sendq1 = sendq.write().await;
        sendq1.insert(Reliability::ReliableOrdered, &buf)?;
        std::mem::drop(sendq1);

        let (user_data_sender, user_data_receiver) = channel::<Vec<u8>>(100);

        let (sender, receiver) = channel::<Vec<u8>>(100);

        let s = Arc::new(s);

        let recv_s = s.clone();
        let connected = Arc::new(tokio::sync::Semaphore::new(0));
        let connected_s = connected.clone();
        let peer_addr = *addr;
        tokio::spawn(async move {
            let mut buf = [0u8; 2048];
            loop {
                if connected_s.is_closed() {
                    break;
                }
                let (size, _) = match match timeout(
                    std::time::Duration::from_secs(10),
                    recv_s.recv_from(&mut buf),
                )
                .await
                {
                    Ok(p) => p,
                    Err(_) => continue,
                } {
                    Ok(p) => p,
                    Err(e) => {
                        #[cfg(target_family = "windows")]
                        if e.raw_os_error().unwrap() == 10040 {
                            // https://docs.microsoft.com/zh-CN/troubleshoot/windows-server/networking/wsaemsgsize-error-10040-in-winsock-2
                            raknet_log_debug!("recv_from error : {}", e.raw_os_error().unwrap());
                            continue;
                        }
                        raknet_log_debug!("recv_from error : {}", e);
                        connected_s.close();
                        break;
                    }
                };

                match sender.send(buf[..size].to_vec()).await {
                    Ok(_) => {}
                    Err(e) => {
                        raknet_log_debug!("channel send error : {}", e);
                        connected_s.close();
                        break;
                    }
                };
            }
            raknet_log_debug!("{} , recv_from finished", peer_addr);
        });

        let (sender_sender, sender_receiver) = channel::<(Vec<u8>, SocketAddr, bool, u8)>(10);

        let ret = RaknetSocket {
            peer_addr: *addr,
            local_addr: s.local_addr().unwrap(),
            user_data_receiver: Arc::new(Mutex::new(user_data_receiver)),
            recvq: Arc::new(Mutex::new(RecvQ::new())),
            sendq,
            close_notifier: connected,
            last_heartbeat_time: Arc::new(AtomicI64::new(cur_timestamp_millis())),
            enable_loss: Arc::new(AtomicBool::new(false)),
            loss_rate: Arc::new(AtomicU8::new(0)),
            incomming_notifier: Arc::new(Notify::new()),
            sender: sender_sender,
            drop_notifier: Arc::new(Notify::new()),
            raknet_version,
        };

        ret.start_receiver(&s, receiver, user_data_sender);
        ret.start_tick(&s, None);
        ret.start_sender(&s, sender_receiver);
        ret.drop_watcher().await;

        raknet_log_debug!("wait incomming notify");
        ret.incomming_notifier.notified().await;

        Ok(ret)
    }

    fn start_receiver(
        &self,
        s: &Arc<UdpSocket>,
        mut receiver: Receiver<Vec<u8>>,
        user_data_sender: Sender<Vec<u8>>,
    ) {
        let connected = self.close_notifier.clone();
        let peer_addr = self.peer_addr;
        let local_addr = self.local_addr;
        let sendq = self.sendq.clone();
        let recvq = self.recvq.clone();
        let last_heartbeat_time = self.last_heartbeat_time.clone();
        let incomming_notify = self.incomming_notifier.clone();
        let s = s.clone();
        let enable_loss = self.enable_loss.clone();
        let loss_rate = self.loss_rate.clone();
        tokio::spawn(async move {
            loop {
                if connected.is_closed() {
                    let mut recvq = recvq.lock().await;
                    for f in recvq.flush(&peer_addr) {
                        RaknetSocket::handle(
                            &f,
                            &peer_addr,
                            &local_addr,
                            &sendq,
                            &user_data_sender,
                            &incomming_notify,
                        )
                        .await
                        .unwrap();
                    }
                    break;
                }

                let buf = match receiver.recv().await {
                    Some(buf) => buf,
                    None => {
                        raknet_log_debug!("channel receiver finished");
                        connected.close();
                        break;
                    }
                };

                last_heartbeat_time.store(cur_timestamp_millis(), Ordering::Relaxed);

                if PacketID::from(buf[0]).unwrap() == PacketID::Disconnect {
                    connected.close();
                    break;
                }

                if buf[0] == PacketID::Ack.to_u8() {
                    //handle ack
                    let mut sendq = sendq.write().await;
                    let ack = read_packet_ack(&buf).unwrap();
                    for i in 0..ack.record_count {
                        if ack.sequences[i as usize].0 == ack.sequences[i as usize].1 {
                            sendq.ack(ack.sequences[i as usize].0, cur_timestamp_millis());
                        } else {
                            for i in ack.sequences[i as usize].0..ack.sequences[i as usize].1 + 1 {
                                sendq.ack(i, cur_timestamp_millis());
                            }
                        }
                    }
                    continue;
                }

                if buf[0] == PacketID::Nack.to_u8() {
                    //handle nack
                    let nack = read_packet_nack(&buf).unwrap();

                    let mut sendq = sendq.write().await;

                    for i in 0..nack.record_count {
                        if nack.sequences[i as usize].0 == nack.sequences[i as usize].1 {
                            sendq.nack(nack.sequences[i as usize].0, cur_timestamp_millis());
                        } else {
                            for i in nack.sequences[i as usize].0..nack.sequences[i as usize].1 + 1
                            {
                                sendq.nack(i, cur_timestamp_millis());
                            }
                        }
                    }
                    continue;
                }

                // handle packet in here
                if buf[0] >= PacketID::FrameSetPacketBegin.to_u8()
                    && buf[0] <= PacketID::FrameSetPacketEnd.to_u8()
                {
                    let frames = FrameVec::new(buf.clone()).unwrap();

                    let mut recvq = recvq.lock().await;
                    let mut is_break = false;
                    for frame in frames.frames {
                        recvq.insert(frame).unwrap();

                        for f in recvq.flush(&peer_addr) {
                            if !RaknetSocket::handle(
                                &f,
                                &peer_addr,
                                &local_addr,
                                &sendq,
                                &user_data_sender,
                                &incomming_notify,
                            )
                            .await
                            .unwrap()
                            {
                                raknet_log_info!("handle over");
                                connected.close();
                                is_break = true;
                            };
                        }

                        if is_break {
                            break;
                        }
                    }

                    //flush ack
                    let acks = recvq.get_ack();

                    if !acks.is_empty() {
                        let packet = Ack {
                            record_count: acks.len() as u16,
                            sequences: acks,
                        };

                        let buf = write_packet_ack(&packet).unwrap();
                        RaknetSocket::sendto(
                            &s,
                            &buf,
                            &peer_addr,
                            enable_loss.load(Ordering::Relaxed),
                            loss_rate.load(Ordering::Relaxed),
                        )
                        .await
                        .unwrap();
                    }
                } else {
                    raknet_log_debug!("unknown packetid : {}", buf[0]);
                }
            }

            raknet_log_debug!("{} , receiver finished", peer_addr);
        });
    }

    fn start_sender(
        &self,
        s: &Arc<UdpSocket>,
        mut receiver: Receiver<(Vec<u8>, SocketAddr, bool, u8)>,
    ) {
        let connected = self.close_notifier.clone();
        let s = s.clone();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    a = receiver.recv() => {
                        match a {
                            Some(p) => {
                                match RaknetSocket::sendto(&s, &p.0, &p.1, p.2, p.3).await{
                                    Ok(_) => {},
                                    Err(e) => {
                                        raknet_log_debug!("sendto error : {}" , e);
                                        break;
                                    },
                                }
                            },
                            None => {
                                raknet_log_debug!("sender worker's receiver channel closed");
                                break;
                            },
                        };
                    },
                    _ = connected.acquire() => {
                        raknet_log_debug!("sender close notified");
                        break;
                    }
                }
            }

            raknet_log_debug!("sender worker closed");
        });
    }

    fn start_tick(&self, s: &Arc<UdpSocket>, collecter: Option<Arc<Mutex<Sender<SocketAddr>>>>) {
        let connected = self.close_notifier.clone();
        let s = s.clone();
        let peer_addr = self.peer_addr;
        let sendq = self.sendq.clone();
        let recvq = self.recvq.clone();
        let mut last_monitor_tick = cur_timestamp_millis();
        let enable_loss = self.enable_loss.clone();
        let loss_rate = self.loss_rate.clone();
        let last_heartbeat_time = self.last_heartbeat_time.clone();
        tokio::spawn(async move {
            loop {
                sleep(std::time::Duration::from_millis(
                    SendQ::DEFAULT_TIMEOUT_MILLS as u64,
                ))
                .await;

                // flush nack
                let mut recvq = recvq.lock().await;
                let nacks = recvq.get_nack();
                if !nacks.is_empty() {
                    let nack = Nack {
                        record_count: nacks.len() as u16,
                        sequences: nacks,
                    };

                    let buf = write_packet_nack(&nack).unwrap();
                    RaknetSocket::sendto(
                        &s,
                        &buf,
                        &peer_addr,
                        enable_loss.load(Ordering::Relaxed),
                        loss_rate.load(Ordering::Relaxed),
                    )
                    .await
                    .unwrap();
                }

                //flush sendq
                let mut sendq = sendq.write().await;
                for f in sendq.flush(cur_timestamp_millis(), &peer_addr) {
                    let data = f.serialize().unwrap();
                    RaknetSocket::sendto(
                        &s,
                        &data,
                        &peer_addr,
                        enable_loss.load(Ordering::Relaxed),
                        loss_rate.load(Ordering::Relaxed),
                    )
                    .await
                    .unwrap();
                }

                //monitor log
                if cur_timestamp_millis() - last_monitor_tick > 10000 {
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
                if cur_timestamp_millis() - last_heartbeat_time.load(Ordering::Relaxed)
                    > RECEIVE_TIMEOUT
                {
                    raknet_log_debug!("recv timeout");
                    connected.close();
                    break;
                }

                if connected.is_closed() {
                    for _ in 0..10 {
                        RaknetSocket::sendto(
                            &s,
                            &[PacketID::Disconnect.to_u8()],
                            &peer_addr,
                            enable_loss.load(Ordering::Relaxed),
                            loss_rate.load(Ordering::Relaxed),
                        )
                        .await
                        .unwrap();
                    }
                    break;
                }
            }

            match collecter {
                Some(p) => {
                    match p.lock().await.send(peer_addr).await {
                        Ok(_) => {}
                        Err(e) => {
                            raknet_log_error!("channel send error : {}", e);
                        }
                    };
                }
                None => {}
            }
            raknet_log_debug!("{} , ticker finished", peer_addr);
        });
    }

    /// Close Raknet Socket.
    /// Normally you don't need to call this method, the RaknetSocket will be closed automatically when it is released.
    /// This method can be called repeatedly.
    ///
    /// # Example
    /// ```ignore
    /// let (latency, motd) = socket::RaknetSocket::ping("127.0.0.1:19132".parse().unwrap()).await.unwrap();
    /// assert!((0..10).contains(&latency));
    /// ```
    pub async fn close(&self) -> Result<()> {
        if !self.close_notifier.is_closed() {
            self.sendq
                .write()
                .await
                .insert(Reliability::Reliable, &[PacketID::Disconnect.to_u8()])?;
            self.close_notifier.close();
        }
        Ok(())
    }

    /// Unconnected ping a Raknet Server and return latency and motd.
    ///
    /// # Example
    /// ```ignore
    /// let (latency, motd) = socket::RaknetSocket::ping("127.0.0.1:19132".parse().unwrap()).await.unwrap();
    /// assert!((0..10).contains(&latency));
    /// ```
    pub async fn ping(addr: &SocketAddr) -> Result<(i64, String)> {
        let s = match UdpSocket::bind("0.0.0.0:0").await {
            Ok(p) => p,
            Err(_) => return Err(RaknetError::BindAddressError),
        };

        loop {
            let packet = PacketUnconnectedPing {
                time: cur_timestamp_millis(),
                magic: true,
                guid: rand::random(),
            };

            let buf = write_packet_ping(&packet)?;

            match s.send_to(buf.as_slice(), addr).await {
                Ok(_) => {}
                Err(e) => {
                    raknet_log_error!("udp socket sendto error {}", e);
                    return Err(RaknetError::SocketError);
                }
            };

            let mut buf = [0u8; 1024];

            match match tokio::time::timeout(
                std::time::Duration::from_secs(5),
                s.recv_from(&mut buf),
            )
            .await
            {
                Ok(p) => p,
                Err(_) => {
                    continue;
                }
            } {
                Ok(p) => p,
                Err(_) => return Err(RaknetError::SocketError),
            };

            if let Ok(p) = read_packet_pong(&buf) {
                return Ok((p.time - packet.time, p.motd));
            };

            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        }
    }

    /// Send a packet
    ///
    /// packet must be `0xfe` as the first byte, using other values of bytes may cause unexpected errors.
    ///
    /// Except Reliability::ReliableOrdered, all other reliability packets must be less than MTU - 60 (default 1340 bytes), otherwise RaknetError::PacketSizeExceedMTU will be returned
    ///
    /// # Example
    /// ```ignore
    /// let socket = RaknetSocket::connect("127.0.0.1:19132".parse().unwrap()).await.unwrap();
    /// socket.send(&[0xfe], Reliability::ReliableOrdered).await.unwrap();
    /// ```
    pub async fn send(&self, buf: &[u8], r: Reliability) -> Result<()> {
        if buf.is_empty() {
            return Err(RaknetError::PacketHeaderError);
        }

        if buf[0] != 0xfe {
            return Err(RaknetError::PacketHeaderError);
        }

        if self.close_notifier.is_closed() {
            return Err(RaknetError::ConnectionClosed);
        }

        //flush sendq
        let mut sendq = self.sendq.write().await;
        sendq.insert(r, buf)?;
        let sender = self.sender.clone();
        for f in sendq.flush(cur_timestamp_millis(), &self.peer_addr) {
            let data = f.serialize().unwrap();
            sender
                .send((
                    data,
                    self.peer_addr,
                    self.enable_loss.load(Ordering::Relaxed),
                    self.loss_rate.load(Ordering::Relaxed),
                ))
                .await
                .unwrap();
        }
        Ok(())
    }

    /// Wait all packet acked
    ///
    /// # Example
    /// ```ignore
    /// let socket = RaknetSocket::connect("127.0.0.1:19132".parse().unwrap()).await.unwrap();
    /// socket.send(&[0xfe], Reliability::ReliableOrdered).await.unwrap();
    /// socket.flush().await.unwrap();
    /// ```
    pub async fn flush(&self) -> Result<()> {
        loop {
            {
                if self.close_notifier.is_closed() {
                    return Err(RaknetError::ConnectionClosed);
                }
                let sendq = self.sendq.read().await;
                if sendq.is_empty() {
                    return Ok(());
                }
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    }

    /// Recv a packet
    ///
    /// # Example
    /// ```ignore
    /// let socket = RaknetSocket::connect("127.0.0.1:19132".parse().unwrap()).await.unwrap();
    /// let buf = socket.recv().await.unwrap();
    /// if buf[0] == 0xfe{
    ///    //do something
    /// }
    /// ```
    pub async fn recv(&self) -> Result<Vec<u8>> {
        match self.user_data_receiver.lock().await.recv().await {
            Some(p) => Ok(p),
            None => {
                if self.close_notifier.is_closed() {
                    return Err(RaknetError::ConnectionClosed);
                }
                Err(RaknetError::SocketError)
            }
        }
    }

    /// Returns the socket address of the remote peer of this Raknet connection.
    ///
    /// # Example
    /// ```ignore
    /// let socket = RaknetSocket::connect("127.0.0.1:19132".parse().unwrap()).await.unwrap();
    /// assert_eq!(socket.peer_addr().unwrap(), SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::new(127, 0, 0, 1), 19132)));
    /// ```
    pub fn peer_addr(&self) -> Result<SocketAddr> {
        Ok(self.peer_addr)
    }

    /// Returns the socket address of the local half of this Raknet connection.
    ///
    /// # Example
    /// ```ignore
    /// let mut socket = RaknetSocket::connect("127.0.0.1:19132".parse().unwrap()).await.unwrap();
    /// assert_eq!(socket.local_addr().unwrap().ip(), IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)));
    /// ```
    pub fn local_addr(&self) -> Result<SocketAddr> {
        Ok(self.local_addr)
    }

    /// return the raknet version used by this connection.
    pub fn raknet_version(&self) -> Result<u8> {
        Ok(self.raknet_version)
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
    pub fn set_loss_rate(&mut self, stage: u8) {
        self.enable_loss.store(true, Ordering::Relaxed);
        self.loss_rate.store(stage, Ordering::Relaxed);
    }

    async fn drop_watcher(&self) {
        let close_notifier = self.close_notifier.clone();
        let drop_notifier = self.drop_notifier.clone();
        tokio::spawn(async move {
            raknet_log_debug!("socket drop watcher start");
            drop_notifier.notify_one();

            drop_notifier.notified().await;

            if close_notifier.is_closed() {
                raknet_log_debug!("socket close notifier closed");
                return;
            }

            close_notifier.close();

            raknet_log_debug!("socket drop watcher closed");
        });

        self.drop_notifier.notified().await;
    }
}

impl Drop for RaknetSocket {
    fn drop(&mut self) {
        self.drop_notifier.notify_one();
    }
}
