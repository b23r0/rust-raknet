use rand::Rng;
use std::{
    net::SocketAddr,
    sync::{
        Arc,
        atomic::{AtomicI64, AtomicU8},
    },
};
use tokio::{
    net::UdpSocket,
    sync::{Mutex, Notify, RwLock, mpsc::channel},
    time::{sleep, timeout},
};

use crate::{
    error::{RaknetError, Result},
    raknet_log_error, raknet_log_info,
};
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::mpsc::{Receiver, Sender};

use crate::{arq::*, packet::*, raknet_log_debug, utils::*};

/// A RakNet connection backed by a UDP socket.
pub struct RaknetSocket {
    udp: std::sync::Weak<UdpSocket>,
    local_addr: SocketAddr,
    peer_addr: SocketAddr,
    user_data_receiver: Arc<Mutex<Receiver<Vec<u8>>>>,
    recvq: Arc<Mutex<RecvQ>>,
    sendq: Arc<RwLock<SendQ>>,
    close_notifier: Arc<tokio::sync::Semaphore>,
    last_heartbeat_time: Arc<AtomicI64>,
    enable_loss: Arc<AtomicBool>,
    loss_rate: Arc<AtomicU8>,
    handshake_complete: Arc<tokio::sync::Semaphore>,
    send_capacity: Arc<Notify>,
    sender: Sender<(Vec<u8>, SocketAddr, bool, u8)>,
    raknet_version: u8,
}

impl RaknetSocket {
    pub(crate) fn close_signal(&self) -> Arc<tokio::sync::Semaphore> {
        self.close_notifier.clone()
    }

    async fn wait_for_handshake(&self) -> Result<()> {
        if self.close_notifier.is_closed() {
            return Err(RaknetError::ConnectionClosed);
        }
        if self.handshake_complete.available_permits() == 0 {
            tokio::select! {
                _ = self.close_notifier.acquire() => return Err(RaknetError::ConnectionClosed),
                permit = self.handshake_complete.acquire() => {
                    drop(permit.map_err(|_| RaknetError::ConnectionClosed)?);
                }
            }
        }
        Ok(())
    }

    /// Create a RakNet socket for an established UDP connection.
    ///
    /// This constructor is used internally by [`RaknetListener`].
    pub async fn from(
        addr: &SocketAddr,
        s: &Arc<UdpSocket>,
        receiver: Receiver<Vec<u8>>,
        mtu: u16,
        collecter: Arc<Mutex<Sender<SocketAddr>>>,
        raknet_version: u8,
    ) -> Self {
        let local_addr = s.local_addr().unwrap_or(*addr);
        let (user_data_sender, user_data_receiver) = channel::<Vec<u8>>(100);
        let (sender_sender, sender_receiver) = channel::<(Vec<u8>, SocketAddr, bool, u8)>(10);

        let ret = RaknetSocket {
            udp: Arc::downgrade(s),
            peer_addr: *addr,
            local_addr,
            user_data_receiver: Arc::new(Mutex::new(user_data_receiver)),
            recvq: Arc::new(Mutex::new(RecvQ::new())),
            sendq: Arc::new(RwLock::new(SendQ::new(mtu))),
            close_notifier: Arc::new(tokio::sync::Semaphore::new(0)),
            last_heartbeat_time: Arc::new(AtomicI64::new(monotonic_millis())),
            enable_loss: Arc::new(AtomicBool::new(false)),
            loss_rate: Arc::new(AtomicU8::new(0)),
            handshake_complete: Arc::new(tokio::sync::Semaphore::new(0)),
            send_capacity: Arc::new(Notify::new()),
            sender: sender_sender,
            raknet_version,
        };
        ret.start_receiver(s, receiver, user_data_sender);
        ret.start_tick(s, Some(collecter));
        ret.start_sender(s, sender_receiver);
        ret
    }

    async fn handle(
        frame: FrameSetPacket,
        peer_addr: &SocketAddr,
        local_addr: &SocketAddr,
        sendq: &RwLock<SendQ>,
        user_data_sender: &Sender<Vec<u8>>,
        handshake_complete: &tokio::sync::Semaphore,
    ) -> Result<bool> {
        let Some(&packet_id) = frame.data.first() else {
            return Err(RaknetError::PacketHeaderError);
        };

        match PacketID::from(packet_id)? {
            PacketID::ConnectionRequest => {
                let packet = read_packet_connection_request(frame.data.as_ref())?;

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
                let packet = read_packet_connection_request_accepted(frame.data.as_ref())?;

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

                // Bedrock sends a connected ping immediately after the connection is accepted.
                let buf = write_packet_connected_ping(&ping)?;
                sendq.insert(Reliability::Unreliable, &buf)?;
                raknet_log_debug!("handshake complete");
                handshake_complete.add_permits(1);
            }
            PacketID::NewIncomingConnection => {
                let _packet = read_packet_new_incomming_connection(frame.data.as_ref())?;
                handshake_complete.add_permits(1);
            }
            PacketID::ConnectedPing => {
                let packet = read_packet_connected_ping(frame.data.as_ref())?;

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
                match user_data_sender.send(frame.data.into()).await {
                    Ok(_) => {}
                    Err(_) => {
                        return Ok(false);
                    }
                };
            }
        }
        Ok(true)
    }

    async fn enqueue_frames(
        frames: Vec<FrameSetPacket>,
        sender: &Sender<(Vec<u8>, SocketAddr, bool, u8)>,
        peer_addr: SocketAddr,
        enable_loss: bool,
        loss_rate: u8,
    ) -> Result<()> {
        for frame in frames {
            let data = frame.serialize()?;
            sender
                .send((data, peer_addr, enable_loss, loss_rate))
                .await
                .map_err(|_| RaknetError::ConnectionClosed)?;
        }
        Ok(())
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
            let loss_rate = loss_rate.min(10);
            let i: u8 = rng.gen_range(0..10);
            if i < loss_rate {
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

    /// Connect to a RakNet server.
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
        // Bedrock peers expect a negative signed GUID on the wire.
        let guid: u64 = rand::random::<u64>() | (1_u64 << 63);

        let s = UdpSocket::bind(if addr.is_ipv4() {
            "0.0.0.0:0"
        } else {
            "[::]:0"
        })
        .await
        .map_err(|_| RaknetError::BindAddressError)?;

        let packet = OpenConnectionRequest1 {
            protocol_version: raknet_version,
            mtu_size: RAKNET_CLIENT_MTU,
        };

        let buf = write_packet_connection_open_request_1(&packet)?;

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

            if size == 0 || src != *addr {
                continue;
            }
            remote_addr = src;
            reply1_size = size;

            if reply1_buf[0] != PacketID::OpenConnectionReply1.to_u8() {
                if reply1_buf[0] == PacketID::IncompatibleProtocolVersion.to_u8() {
                    let _packet =
                        match read_packet_incompatible_protocol_version(&reply1_buf[..size]) {
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
            address: remote_addr,
            mtu: reply1.mtu_size,
            guid,
        };

        let buf = write_packet_connection_open_request_2(&packet)?;

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
            let (size, source) =
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

            if size == 0 || source != *addr {
                continue;
            }

            if buf[0] == PacketID::OpenConnectionReply1.to_u8() {
                raknet_log_debug!("repeat receive reply1");
                continue;
            }

            if matches!(
                PacketID::from(buf[0]),
                Ok(PacketID::AlreadyConnected | PacketID::Disconnect)
            ) {
                return Err(RaknetError::ConnectionClosed);
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

        let buf = write_packet_connection_request(&packet)?;

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
                let received = tokio::select! {
                    _ = connected_s.acquire() => break,
                    result = recv_s.recv_from(&mut buf) => result,
                };
                let (size, source) = match received {
                    Ok(p) => p,
                    Err(e) => {
                        #[cfg(target_family = "windows")]
                        if e.raw_os_error() == Some(10040) {
                            // Windows reports WSAEMSGSIZE when a datagram exceeds the buffer.
                            raknet_log_debug!("recv_from error : {}", 10040);
                            continue;
                        }
                        raknet_log_debug!("recv_from error : {}", e);
                        connected_s.close();
                        break;
                    }
                };

                if size == 0 || source != peer_addr {
                    continue;
                }

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
            udp: Arc::downgrade(&s),
            peer_addr: *addr,
            local_addr: s.local_addr().map_err(|_| RaknetError::SocketError)?,
            user_data_receiver: Arc::new(Mutex::new(user_data_receiver)),
            recvq: Arc::new(Mutex::new(RecvQ::new())),
            sendq,
            close_notifier: connected,
            last_heartbeat_time: Arc::new(AtomicI64::new(monotonic_millis())),
            enable_loss: Arc::new(AtomicBool::new(false)),
            loss_rate: Arc::new(AtomicU8::new(0)),
            handshake_complete: Arc::new(tokio::sync::Semaphore::new(0)),
            send_capacity: Arc::new(Notify::new()),
            sender: sender_sender,
            raknet_version,
        };

        ret.start_receiver(&s, receiver, user_data_sender);
        ret.start_tick(&s, None);
        ret.start_sender(&s, sender_receiver);

        raknet_log_debug!("waiting for handshake");
        ret.wait_for_handshake().await?;

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
        let sender = self.sender.clone();
        let last_heartbeat_time = self.last_heartbeat_time.clone();
        let handshake_complete = self.handshake_complete.clone();
        let send_capacity = self.send_capacity.clone();
        let s = s.clone();
        let enable_loss = self.enable_loss.clone();
        let loss_rate = self.loss_rate.clone();
        tokio::spawn(async move {
            // Application backpressure must not prevent ACK/NACK processing.
            let mut pending_data = std::collections::VecDeque::<Vec<u8>>::new();
            let mut pending_bytes = 0usize;
            loop {
                let buf = if pending_data.is_empty() {
                    tokio::select! {
                        _ = connected.acquire() => break,
                        message = receiver.recv() => match message {
                            Some(buf) => buf,
                            None => { connected.close(); break; }
                        }
                    }
                } else {
                    tokio::select! {
                        biased;
                        _ = connected.acquire() => break,
                        permit = user_data_sender.reserve() => {
                            let Ok(permit) = permit else { connected.close(); break; };
                            if let Some(data) = pending_data.pop_front() {
                                pending_bytes -= data.len();
                                permit.send(data);
                            }
                            continue;
                        }
                        message = receiver.recv() => match message {
                            Some(buf) => buf,
                            None => { connected.close(); break; }
                        }
                    }
                };
                let Some(&packet_id) = buf.first() else {
                    continue;
                };

                last_heartbeat_time.store(monotonic_millis(), Ordering::Relaxed);
                let packet_kind = match PacketID::from(packet_id) {
                    Ok(kind) => kind,
                    Err(error) => {
                        raknet_log_debug!("ignoring packet with invalid ID: {:?}", error);
                        continue;
                    }
                };

                if packet_kind == PacketID::Disconnect {
                    connected.close();
                    break;
                }

                if packet_kind == PacketID::Ack {
                    let ack = match read_packet_ack(&buf) {
                        Ok(ack) => ack,
                        Err(error) => {
                            raknet_log_debug!("ignoring malformed ACK: {}", error);
                            continue;
                        }
                    };
                    let now = monotonic_millis();
                    let outgoing_frames = {
                        let mut sendq = sendq.write().await;
                        sendq.ack_ranges(&ack.sequences, now);
                        sendq.flush(now, &peer_addr)
                    };
                    send_capacity.notify_waiters();
                    if let Err(error) = RaknetSocket::enqueue_frames(
                        outgoing_frames,
                        &sender,
                        peer_addr,
                        enable_loss.load(Ordering::Relaxed),
                        loss_rate.load(Ordering::Relaxed),
                    )
                    .await
                    {
                        raknet_log_debug!("failed to queue frames after ACK: {}", error);
                    }
                    continue;
                }

                if packet_kind == PacketID::Nack {
                    let nack = match read_packet_nack(&buf) {
                        Ok(nack) => nack,
                        Err(error) => {
                            raknet_log_debug!("ignoring malformed NACK: {}", error);
                            continue;
                        }
                    };
                    let now = monotonic_millis();
                    let outgoing_frames = {
                        let mut sendq = sendq.write().await;
                        sendq.nack_ranges(&nack.sequences, now);
                        sendq.flush(now, &peer_addr)
                    };
                    if let Err(error) = RaknetSocket::enqueue_frames(
                        outgoing_frames,
                        &sender,
                        peer_addr,
                        enable_loss.load(Ordering::Relaxed),
                        loss_rate.load(Ordering::Relaxed),
                    )
                    .await
                    {
                        raknet_log_debug!("failed to queue frames after NACK: {}", error);
                    }
                    continue;
                }

                if packet_kind != PacketID::FrameSetPacketBegin {
                    raknet_log_debug!("ignoring unsupported packet ID: {packet_id}");
                    continue;
                }

                // Leave excess datagrams unacknowledged so reliable senders retry.
                // Already accepted data remains owned until delivered or disconnected.
                if pending_bytes >= 1024 * 1024 || pending_data.len() >= 4096 {
                    continue;
                }

                let frames = match FrameVec::new(&buf) {
                    Ok(frames) => frames,
                    Err(error) => {
                        raknet_log_debug!("ignoring malformed frame set: {}", error);
                        continue;
                    }
                };
                let (ready_frames, acks, invalid) = {
                    let mut recvq = recvq.lock().await;
                    let mut ready_frames = Vec::new();
                    let mut invalid = false;
                    for frame in frames.frames {
                        if let Err(error) = recvq.insert(frame) {
                            raknet_log_debug!(
                                "receive window or reassembly rejected frame: {}",
                                error
                            );
                            invalid = true;
                            break;
                        }
                        if ready_frames.is_empty() {
                            ready_frames = recvq.flush(&peer_addr);
                        } else {
                            ready_frames.extend(recvq.flush(&peer_addr));
                        }
                    }
                    (ready_frames, recvq.get_ack(), invalid)
                };

                if invalid {
                    connected.close();
                    break;
                }
                let mut should_close = false;
                for frame in ready_frames {
                    if frame.data.first() == Some(&PacketID::Game.to_u8()) {
                        let data: Vec<u8> = frame.data.into();
                        // There is a single application-channel producer. Try the
                        // common path without registering another async waiter.
                        let data = if pending_data.is_empty() {
                            match user_data_sender.try_send(data) {
                                Ok(()) => continue,
                                Err(tokio::sync::mpsc::error::TrySendError::Full(data)) => data,
                                Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => {
                                    connected.close();
                                    should_close = true;
                                    break;
                                }
                            }
                        } else {
                            data
                        };
                        pending_bytes += data.len();
                        pending_data.push_back(data);
                        continue;
                    }
                    let result = tokio::select! {
                        _ = connected.acquire() => break,
                        result = RaknetSocket::handle(
                        frame,
                        &peer_addr,
                        &local_addr,
                        &sendq,
                        &user_data_sender,
                        &handshake_complete,
                    )
                        => result,
                    };
                    match result {
                        Ok(true) => {}
                        Ok(false) => {
                            raknet_log_info!("peer disconnected");
                            connected.close();
                            should_close = true;
                            break;
                        }
                        Err(error) => {
                            raknet_log_debug!("dropping frame: {}", error);
                        }
                    }
                }

                if !acks.is_empty() {
                    let record_count = match u16::try_from(acks.len()) {
                        Ok(count) => count,
                        Err(_) => {
                            raknet_log_error!("too many ACK ranges to encode");
                            continue;
                        }
                    };
                    let packet = Ack {
                        record_count,
                        sequences: acks,
                    };
                    match write_packet_ack(&packet) {
                        Ok(packet) => {
                            if let Err(error) = RaknetSocket::sendto(
                                &s,
                                &packet,
                                &peer_addr,
                                enable_loss.load(Ordering::Relaxed),
                                loss_rate.load(Ordering::Relaxed),
                            )
                            .await
                            {
                                raknet_log_error!("failed to send ACK: {}", error);
                            }
                        }
                        Err(error) => raknet_log_error!("failed to encode ACK: {}", error),
                    }
                }

                if should_close {
                    break;
                }
            }

            raknet_log_debug!("{} receive worker closed", peer_addr);
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
        let mut last_monitor_tick = monotonic_millis();
        let enable_loss = self.enable_loss.clone();
        let loss_rate = self.loss_rate.clone();
        let last_heartbeat_time = self.last_heartbeat_time.clone();
        tokio::spawn(async move {
            loop {
                sleep(std::time::Duration::from_millis(
                    SendQ::DEFAULT_TIMEOUT_MILLS as u64,
                ))
                .await;

                // Send NACK ranges.
                let nacks = {
                    let mut recvq = recvq.lock().await;
                    if recvq.fragments_expired() {
                        connected.close();
                        break;
                    }
                    recvq.get_nack()
                };
                if !nacks.is_empty() {
                    let Ok(record_count) = u16::try_from(nacks.len()) else {
                        raknet_log_error!("too many NACK ranges to encode");
                        continue;
                    };
                    let nack = Nack {
                        record_count,
                        sequences: nacks,
                    };

                    match write_packet_nack(&nack) {
                        Ok(packet) => {
                            if let Err(error) = RaknetSocket::sendto(
                                &s,
                                &packet,
                                &peer_addr,
                                enable_loss.load(Ordering::Relaxed),
                                loss_rate.load(Ordering::Relaxed),
                            )
                            .await
                            {
                                raknet_log_error!("failed to send NACK: {}", error);
                            }
                        }
                        Err(error) => raknet_log_error!("failed to encode NACK: {}", error),
                    }
                }

                // Send queued frames.
                let outgoing_frames = {
                    let mut sendq = sendq.write().await;
                    sendq.flush(monotonic_millis(), &peer_addr)
                };
                for frame in outgoing_frames {
                    let data = match frame.serialize() {
                        Ok(data) => data,
                        Err(error) => {
                            raknet_log_error!("failed to encode frame: {}", error);
                            continue;
                        }
                    };
                    if let Err(error) = RaknetSocket::sendto(
                        &s,
                        &data,
                        &peer_addr,
                        enable_loss.load(Ordering::Relaxed),
                        loss_rate.load(Ordering::Relaxed),
                    )
                    .await
                    {
                        raknet_log_error!("failed to send frame: {}", error);
                    }
                }

                // Periodically report queue and latency state.
                if monotonic_millis() - last_monitor_tick > 10000 {
                    let (send_queue_size, sent_queue_size, rto) = {
                        let sendq = sendq.read().await;
                        (
                            sendq.get_reliable_queue_size(),
                            sendq.get_sent_queue_size(),
                            sendq.get_rto(),
                        )
                    };
                    let (recvq_size, fragment_size, ordered_size, ordered_keys) = {
                        let recvq = recvq.lock().await;
                        (
                            recvq.get_size(),
                            recvq.get_fragment_queue_size(),
                            recvq.get_ordered_packet(),
                            recvq.get_ordered_keys(),
                        )
                    };
                    raknet_log_debug!(
                        "peer addr: {} | send queue: {} | sent queue: {} | RTO: {} | receive queue: {} | fragments: {} | ordered queue: {} - {:?}",
                        peer_addr,
                        send_queue_size,
                        sent_queue_size,
                        rto,
                        recvq_size,
                        fragment_size,
                        ordered_size,
                        ordered_keys
                    );
                    last_monitor_tick = monotonic_millis();
                }

                // Close the connection after 60 seconds without receiving a packet.
                if monotonic_millis() - last_heartbeat_time.load(Ordering::Relaxed)
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
                        .ok();
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

    /// Close the RakNet connection.
    /// The connection also closes when the socket is dropped. This method is idempotent.
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

    /// Ping a RakNet server and return the round-trip time and MOTD.
    ///
    /// # Example
    /// ```ignore
    /// let (latency, motd) = socket::RaknetSocket::ping("127.0.0.1:19132".parse().unwrap()).await.unwrap();
    /// assert!((0..10).contains(&latency));
    /// ```
    pub async fn ping(addr: &SocketAddr) -> Result<(i64, String)> {
        let s = UdpSocket::bind(if addr.is_ipv4() {
            "0.0.0.0:0"
        } else {
            "[::]:0"
        })
        .await
        .map_err(|_| RaknetError::BindAddressError)?;

        loop {
            let packet = PacketUnconnectedPing {
                time: cur_timestamp_millis(),
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

    /// Send a Bedrock game packet.
    ///
    /// The packet must begin with `0xfe`. `ReliableOrdered` messages are fragmented
    /// when necessary; other modes must fit the negotiated MTU. Messages exceeding
    /// the 64 MiB send budget (including fragment overhead) return
    /// [`RaknetError::PacketSizeExceedMTU`]. A full send queue applies asynchronous
    /// backpressure until capacity is available or the connection closes.
    ///
    /// # Example
    /// ```ignore
    /// let socket = RaknetSocket::connect("127.0.0.1:19132".parse().unwrap()).await.unwrap();
    /// socket.send(&[0xfe], Reliability::ReliableOrdered).await.unwrap();
    /// ```
    pub async fn send(&self, buf: &[u8], reliability: Reliability) -> Result<()> {
        self.send_with_order_channel(buf, reliability, 0).await
    }

    /// Send a packet on a specific RakNet ordering channel.
    ///
    /// Each ordering channel has an independent reliable-ordered sequence.
    /// The channel is relevant to ordered and sequenced reliability modes;
    /// unordered modes do not carry an ordering channel on the wire.
    pub async fn send_with_order_channel(
        &self,
        buf: &[u8],
        reliability: Reliability,
        order_channel: u8,
    ) -> Result<()> {
        if buf.is_empty() {
            return Err(RaknetError::PacketHeaderError);
        }

        if buf[0] != 0xfe {
            return Err(RaknetError::PacketHeaderError);
        }

        if self.close_notifier.is_closed() {
            return Err(RaknetError::ConnectionClosed);
        }

        // Offline negotiation can create the server socket before the connected
        // handshake arrives. Do not let application data overtake that handshake.
        if self.handshake_complete.available_permits() == 0 {
            self.wait_for_handshake().await?;
        }
        loop {
            let frames = {
                let mut sendq = self.sendq.write().await;
                if sendq.has_capacity(reliability, buf.len())? {
                    sendq.insert_with_order_channel(reliability, buf, order_channel)?;
                    Some(sendq.flush(monotonic_millis(), &self.peer_addr))
                } else {
                    None
                }
            };
            if let Some(frames) = frames {
                // Send from the caller task to avoid an extra channel hop on
                // the application path. Reliable frames remain in the send queue
                // until acknowledged, including when this future is cancelled.
                let udp = self.udp.upgrade().ok_or(RaknetError::ConnectionClosed)?;
                let enable_loss = self.enable_loss.load(Ordering::Relaxed);
                let loss_rate = self.loss_rate.load(Ordering::Relaxed);
                for frame in frames {
                    let data = frame.serialize()?;
                    Self::sendto(&udp, &data, &self.peer_addr, enable_loss, loss_rate)
                        .await
                        .map_err(|_| RaknetError::SocketError)?;
                }
                return Ok(());
            }
            let available = self.send_capacity.notified();
            tokio::pin!(available);
            available.as_mut().enable();
            // Register before rechecking capacity so an intervening ACK cannot
            // be missed. No polling delay is added to a newly available slot.
            if self
                .sendq
                .read()
                .await
                .has_capacity(reliability, buf.len())?
            {
                continue;
            }
            tokio::select! {
                _ = self.close_notifier.acquire() => return Err(RaknetError::ConnectionClosed),
                _ = &mut available => {}
            }
        }
    }

    /// Wait until all reliable packets have been acknowledged.
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

    /// Receive the next game packet.
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

    /// Return the remote peer's UDP address.
    ///
    /// # Example
    /// ```ignore
    /// let socket = RaknetSocket::connect("127.0.0.1:19132".parse().unwrap()).await.unwrap();
    /// assert_eq!(socket.peer_addr().unwrap(), SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::new(127, 0, 0, 1), 19132)));
    /// ```
    pub fn peer_addr(&self) -> Result<SocketAddr> {
        Ok(self.peer_addr)
    }

    /// Return the local UDP address used by this connection.
    ///
    /// # Example
    /// ```ignore
    /// let mut socket = RaknetSocket::connect("127.0.0.1:19132".parse().unwrap()).await.unwrap();
    /// assert_eq!(socket.local_addr().unwrap().ip(), IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)));
    /// ```
    pub fn local_addr(&self) -> Result<SocketAddr> {
        Ok(self.local_addr)
    }

    /// Return the RakNet protocol version used by this connection.
    pub fn raknet_version(&self) -> Result<u8> {
        Ok(self.raknet_version)
    }

    /// Enable simulated packet loss for testing.
    ///
    /// `stage` ranges from 0 (no loss) to 10 (all packets dropped).
    /// # Example
    /// ```ignore
    /// let mut socket = RaknetSocket::connect("127.0.0.1:19132".parse().unwrap()).await.unwrap();
    /// // Simulate 20% packet loss.
    /// socket.set_loss_rate(2);
    /// ```
    pub fn set_loss_rate(&mut self, stage: u8) {
        self.enable_loss.store(true, Ordering::Relaxed);
        self.loss_rate.store(stage, Ordering::Relaxed);
    }
}

impl Drop for RaknetSocket {
    fn drop(&mut self) {
        self.close_notifier.close();
    }
}
