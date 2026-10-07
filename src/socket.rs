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

const MAINTENANCE_IDLE_MILLIS: i64 = 500;

#[cfg_attr(not(feature = "recovery-policy"), derive(Default))]
struct Maintenance {
    wakeup: Notify,
    idle: AtomicBool,
    enabled: AtomicBool,
    #[cfg(feature = "recovery-policy")]
    deadline: AtomicI64,
    #[cfg(feature = "recovery-policy")]
    deadline_driven: AtomicBool,
    #[cfg(feature = "recovery-policy")]
    wake_at: AtomicI64,
}

#[cfg(feature = "recovery-policy")]
impl Default for Maintenance {
    fn default() -> Self {
        Self {
            wakeup: Notify::new(),
            idle: AtomicBool::new(false),
            enabled: AtomicBool::new(false),
            deadline: AtomicI64::new(i64::MAX),
            deadline_driven: AtomicBool::new(false),
            wake_at: AtomicI64::new(i64::MAX),
        }
    }
}

#[cfg(feature = "recovery-policy")]
impl Maintenance {
    fn publish(&self, queue: &SendQ, wake: bool) {
        if !self.deadline_driven.load(Ordering::Relaxed) {
            return;
        }
        let deadline = queue.recovery_deadline();
        self.deadline.store(deadline, Ordering::Release);
        // A new flight need not interrupt a periodic wakeup that already occurs
        // before its retry deadline. Compare against the armed timer, not an ACK
        // transiently clearing the queue's cached deadline.
        if wake && deadline < self.wake_at.load(Ordering::Acquire) {
            self.wakeup.notify_one();
        }
    }
}

enum DatagramInput {
    Channel {
        receiver: Receiver<Vec<u8>>,
        packet: Vec<u8>,
    },
    Direct(Box<DirectDatagrams>),
}

struct DirectDatagrams {
    socket: Arc<UdpSocket>,
    peer: SocketAddr,
    packet: Box<[u8; 2048]>,
    queued: Box<[u8; 2048]>,
    length: usize,
    queued_length: usize,
    error: Option<std::io::Error>,
}

impl From<Receiver<Vec<u8>>> for DatagramInput {
    fn from(receiver: Receiver<Vec<u8>>) -> Self {
        Self::Channel {
            receiver,
            packet: Vec::new(),
        }
    }
}

impl DatagramInput {
    fn decode_into(&mut self, frames: &mut Vec<FrameSetPacket>) -> Result<()> {
        match self {
            Self::Channel { packet, .. } => {
                FrameVec::decode_owned_into(std::mem::take(packet), frames)
            }
            Self::Direct(input) => FrameVec::decode_into(&input.packet[..input.length], frames),
        }
    }

    fn direct(socket: Arc<UdpSocket>, peer: SocketAddr) -> Self {
        Self::Direct(Box::new(DirectDatagrams {
            socket,
            peer,
            packet: Box::new([0; 2048]),
            queued: Box::new([0; 2048]),
            length: 0,
            queued_length: 0,
            error: None,
        }))
    }

    fn enable_fragment_worker(&mut self, close: Arc<tokio::sync::Semaphore>) {
        if !matches!(self, Self::Direct(_)) {
            return;
        }
        // Preserve the previous client's bounded queue for sustained reassembly.
        // Small-frame clients keep the fused path and allocate no receive task.
        let (sender, receiver) = channel(100);
        let Self::Direct(input) = std::mem::replace(self, receiver.into()) else {
            unreachable!();
        };
        let DirectDatagrams {
            socket,
            peer,
            mut packet,
            queued,
            queued_length,
            error,
            ..
        } = *input;
        tokio::spawn(async move {
            if error.is_some() {
                close.close();
                return;
            }
            if queued_length != 0 && sender.send(queued[..queued_length].to_vec()).await.is_err() {
                return;
            }
            drop(queued);
            loop {
                let received = tokio::select! {
                    _ = close.acquire() => break,
                    result = socket.recv_from(packet.as_mut()) => result,
                };
                let (length, source) = match received {
                    Ok(received) => received,
                    Err(error) => {
                        #[cfg(target_family = "windows")]
                        if error.raw_os_error() == Some(10040) {
                            continue;
                        }
                        raknet_log_debug!("fragment receiver failed: {error}");
                        close.close();
                        break;
                    }
                };
                if length == 0 || source != peer {
                    continue;
                }
                let data = packet[..length].to_vec();
                tokio::select! {
                    _ = close.acquire() => break,
                    result = sender.send(data) => {
                        if result.is_err() {
                            break;
                        }
                    }
                }
            }
        });
    }

    async fn recv(&mut self) -> std::io::Result<bool> {
        match self {
            Self::Channel { receiver, packet } => {
                let Some(received) = receiver.recv().await else {
                    return Ok(false);
                };
                *packet = received;
                Ok(true)
            }
            Self::Direct(input) => {
                if let Some(error) = input.error.take() {
                    return Err(error);
                }
                if input.queued_length != 0 {
                    // Buffered reads still consume a cooperative task budget.
                    tokio::task::consume_budget().await;
                    std::mem::swap(&mut input.packet, &mut input.queued);
                    input.length = std::mem::take(&mut input.queued_length);
                    return Ok(true);
                }
                loop {
                    let result = input.socket.recv_from(input.packet.as_mut()).await;
                    match result {
                        Ok((length, source)) if source == input.peer && length != 0 => {
                            input.length = length;
                            return Ok(true);
                        }
                        Ok(_) => {}
                        Err(error) => {
                            #[cfg(target_family = "windows")]
                            if error.raw_os_error() == Some(10040) {
                                continue;
                            }
                            return Err(error);
                        }
                    }
                }
            }
        }
    }

    fn packet(&self) -> &[u8] {
        match self {
            Self::Channel { packet, .. } => packet,
            Self::Direct(input) => &input.packet[..input.length],
        }
    }

    fn should_flush_ack(&mut self) -> bool {
        match self {
            Self::Channel { receiver, .. } => receiver.is_empty(),
            Self::Direct(input) => {
                if input.queued_length != 0 {
                    return false;
                }
                if input.error.is_some() {
                    return true;
                }
                // Read ahead only when a datagram is already available. ACKs
                // never wait for another packet or a timer. Bound foreign traffic.
                for _ in 0..32 {
                    match input.socket.try_recv_from(input.queued.as_mut()) {
                        Ok((length, source)) if source == input.peer && length != 0 => {
                            input.queued_length = length;
                            return false;
                        }
                        Ok(_) => {}
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            return true;
                        }
                        Err(error) => {
                            #[cfg(target_family = "windows")]
                            if error.raw_os_error() == Some(10040) {
                                continue;
                            }
                            input.error = Some(error);
                            return true;
                        }
                    }
                }
                true
            }
        }
    }
}

#[cfg(test)]
async fn wait_for_read_ahead(input: &mut DatagramInput) {
    let DatagramInput::Direct(direct) = input else {
        panic!("read-ahead readiness requires direct datagram input");
    };
    let socket = direct.socket.clone();
    timeout(std::time::Duration::from_secs(2), async {
        // A completed UDP send does not imply that Tokio has observed readable
        // readiness. Poll the nonblocking prefetch, then await the real event.
        while input.should_flush_ack() {
            let DatagramInput::Direct(direct) = input else {
                unreachable!();
            };
            assert!(
                direct.error.is_none(),
                "read-ahead failed: {:?}",
                direct.error
            );
            socket.readable().await.unwrap();
        }
    })
    .await
    .expect("peer datagram did not become available for read-ahead");
}

enum ControlAction {
    Continue,
    HandshakeComplete,
    Disconnect,
}

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
    maintenance: Arc<Maintenance>,
    raknet_version: u8,
    mtu: u16,
}

impl RaknetSocket {
    /// Pause maintenance after 500 ms of quiet traffic and empty queues.
    ///
    /// Disabled by default to preserve the periodic schedule under load.
    /// This can save CPU with many idle peers, but changes maintenance timing.
    /// Sending or receiving work wakes a paused connection. Disabling this
    /// option immediately resumes periodic maintenance.
    pub fn set_idle_maintenance(&self, enabled: bool) {
        self.maintenance.enabled.store(enabled, Ordering::Release);
        if !enabled {
            self.maintenance.wakeup.notify_one();
        }
    }

    /// Return the negotiated nominal RakNet MTU in bytes.
    pub fn mtu(&self) -> u16 {
        self.mtu
    }

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
    /// This constructor is used internally by [`crate::RaknetListener`].
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
            maintenance: Arc::new(Maintenance::default()),
            raknet_version,
            mtu,
        };
        ret.start_receiver(s, receiver, user_data_sender);
        ret.start_tick(s, Some(collecter));
        ret
    }

    async fn handle(
        frame: FrameSetPacket,
        peer_addr: &SocketAddr,
        local_addr: &SocketAddr,
        sendq: &RwLock<SendQ>,
        user_data_sender: &Sender<Vec<u8>>,
    ) -> Result<ControlAction> {
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
                return Ok(ControlAction::HandshakeComplete);
            }
            PacketID::NewIncomingConnection => {
                let _packet = read_packet_new_incomming_connection(frame.data.as_ref())?;
                return Ok(ControlAction::HandshakeComplete);
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
                return Ok(ControlAction::Disconnect);
            }
            _ => {
                match user_data_sender.send(frame.data.into()).await {
                    Ok(_) => {}
                    Err(_) => {
                        return Ok(ControlAction::Disconnect);
                    }
                };
            }
        }
        Ok(ControlAction::Continue)
    }

    async fn send_ack_ranges(
        socket: &UdpSocket,
        sequences: &[(u32, u32)],
        packet: &mut Vec<u8>,
        peer: &SocketAddr,
        enable_loss: bool,
        loss_rate: u8,
    ) -> Result<()> {
        if sequences.is_empty() {
            return Ok(());
        }
        write_control_ranges_into(PacketID::Ack, sequences, packet)?;
        Self::sendto(socket, packet, peer, enable_loss, loss_rate)
            .await
            .map(|_| ())
            .map_err(|_| RaknetError::SocketError)
    }

    async fn transmit_frames(
        socket: &UdpSocket,
        frames: OutgoingFrames,
        peer: &SocketAddr,
        enable_loss: bool,
        loss_rate: u8,
    ) -> Result<()> {
        if frames.len() <= 1 {
            for frame in &frames {
                Self::send_frame(socket, frame, peer, enable_loss, loss_rate).await?;
            }
            return Ok(());
        }
        if frames.coalesced {
            let mut wire = Vec::new();
            let mut start = 0;
            while start < frames.len() {
                let mut end = start + 1;
                while end < frames.len()
                    && frames[end].sequence_number == frames[start].sequence_number
                {
                    end += 1;
                }
                FrameSetPacket::serialize_group_into(&frames[start..end], &mut wire)?;
                Self::sendto(socket, &wire, peer, enable_loss, loss_rate)
                    .await
                    .map_err(|_| RaknetError::SocketError)?;
                start = end;
            }
            return Ok(());
        }
        #[cfg(target_os = "linux")]
        if frames.len() > 1 && !enable_loss {
            if let Err(error) = crate::udp::send_frames(socket, &frames, peer).await {
                raknet_log_error!("UDP batch send failed: {}", error);
            }
            return Ok(());
        }
        // Reuse one contiguous buffer on platforms without batched syscalls.
        let mut packet = Vec::new();
        for frame in &frames {
            frame.serialize_into(&mut packet)?;
            Self::sendto(socket, &packet, peer, enable_loss, loss_rate)
                .await
                .map_err(|_| RaknetError::SocketError)?;
        }
        Ok(())
    }

    async fn transmit_replies(
        socket: &Arc<UdpSocket>,
        frames: OutgoingFrames,
        peer: &SocketAddr,
        close: &Arc<tokio::sync::Semaphore>,
        bulk_sender: &mut Option<Sender<(Vec<u8>, bool, u8)>>,
        enable_loss: bool,
        loss_rate: u8,
    ) -> Result<()> {
        // A fragment batch can keep the flight window full even when it is small.
        // Delegate it and larger queue drains so ACK reception can progress.
        // Ordinary connections allocate no send task until delegation is needed.
        if frames.coalesced {
            return Self::transmit_frames(socket, frames, peer, enable_loss, loss_rate).await;
        }
        if bulk_sender.is_none()
            && frames.len() <= 8
            && frames.iter().all(|frame| !frame.is_fragment())
        {
            return Self::transmit_frames(socket, frames, peer, enable_loss, loss_rate).await;
        }
        let sender = bulk_sender.get_or_insert_with(|| {
            let (sender, mut receiver) = channel::<(Vec<u8>, bool, u8)>(10);
            let socket = socket.clone();
            let close = close.clone();
            let peer = *peer;
            tokio::spawn(async move {
                loop {
                    tokio::select! {
                        packet = receiver.recv() => {
                            let Some((data, enable_loss, loss_rate)) = packet else { break; };
                            if Self::sendto(&socket, &data, &peer, enable_loss, loss_rate).await.is_err() {
                                break;
                            }
                        }
                        _ = close.acquire() => break,
                    }
                }
            });
            sender
        });
        for frame in &frames {
            // Queued datagrams own only their wire bytes, rather than retaining
            // an entire fragmented application's shared backing allocation.
            sender
                .send((frame.serialize()?, enable_loss, loss_rate))
                .await
                .map_err(|_| RaknetError::ConnectionClosed)?;
        }
        Ok(())
    }

    async fn send_frame(
        socket: &UdpSocket,
        frame: &FrameSetPacket,
        target: &SocketAddr,
        enable_loss: bool,
        loss_rate: u8,
    ) -> Result<()> {
        // The slices form one datagram; they are never sent independently.
        #[cfg(not(any(target_os = "redox", target_os = "wasi", target_os = "horizon")))]
        {
            let mut header = [0; 32];
            let length = frame.encode_header(&mut header)?;
            if frame.data.len() <= 128 {
                // Tiny frames use a contiguous stack buffer to avoid allocation
                // and the extra syscall overhead of vectored I/O.
                let mut packet = [0; 160];
                packet[..length].copy_from_slice(&header[..length]);
                let end = length + frame.data.len();
                packet[length..end].copy_from_slice(&frame.data);
                return Self::sendto(socket, &packet[..end], target, enable_loss, loss_rate)
                    .await
                    .map(|_| ())
                    .map_err(|_| RaknetError::SocketError);
            }
            if enable_loss && rand::thread_rng().gen_range(0..10) < loss_rate.min(10) {
                return Ok(());
            }
            let slices = [
                std::io::IoSlice::new(&header[..length]),
                std::io::IoSlice::new(&frame.data),
            ];
            let address = socket2::SockAddr::from(*target);
            let result = socket
                .async_io(tokio::io::Interest::WRITABLE, || {
                    loop {
                        match socket2::SockRef::from(socket).send_to_vectored(&slices, &address) {
                            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {
                                continue;
                            }
                            result => return result,
                        }
                    }
                })
                .await;
            if let Err(error) = result {
                // Reliable frames remain pending for the normal retry path.
                raknet_log_error!("udp socket send_to error: {}", error);
            }
            Ok(())
        }
        #[cfg(any(target_os = "redox", target_os = "wasi", target_os = "horizon"))]
        {
            let data = frame.serialize()?;
            Self::sendto(socket, &data, target, enable_loss, loss_rate)
                .await
                .map(|_| ())
                .map_err(|_| RaknetError::SocketError)
        }
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
        Self::connect_with_version_and_mtu(addr, raknet_version, RAKNET_CLIENT_MTU).await
    }

    /// Connect with an explicit RakNet version and nominal MTU in bytes.
    ///
    /// The requested MTU must be between 61 and 1,492 bytes. The server may
    /// negotiate a smaller value. Choose a size the network path can carry;
    /// the ordinary connection APIs retain their 1,400-byte default.
    pub async fn connect_with_version_and_mtu(
        addr: &SocketAddr,
        raknet_version: u8,
        requested_mtu: u16,
    ) -> Result<Self> {
        if !(61..=RAKNET_MAX_MTU).contains(&requested_mtu) {
            return Err(RaknetError::PacketSizeExceedMTU);
        }
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
            mtu_size: requested_mtu,
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

        if !(61..=requested_mtu).contains(&reply1.mtu_size) {
            return Err(RaknetError::IncorrectReply);
        }

        let packet = OpenConnectionRequest2 {
            address: remote_addr,
            mtu: reply1.mtu_size,
            guid,
        };

        let buf = write_packet_connection_open_request_2(&packet)?;

        let negotiated_mtu = loop {
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

            let reply2 = match read_packet_connection_open_reply_2(&buf[..size]) {
                Ok(p) => p,
                Err(_) => return Err(RaknetError::PacketParseError),
            };

            if !(61..=reply1.mtu_size).contains(&reply2.mtu) {
                return Err(RaknetError::IncorrectReply);
            }
            break reply2.mtu;
        };

        let sendq = Arc::new(RwLock::new(SendQ::new(negotiated_mtu)));

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

        let s = Arc::new(s);
        let connected = Arc::new(tokio::sync::Semaphore::new(0));
        let receiver = DatagramInput::direct(s.clone(), *addr);

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
            maintenance: Arc::new(Maintenance::default()),
            raknet_version,
            mtu: negotiated_mtu,
        };

        ret.start_receiver(&s, receiver, user_data_sender);
        ret.start_tick(&s, None);

        // Start the connected handshake immediately; the ticker remains the
        // fallback for reliable retransmission rather than the initial sender.
        let frames = {
            let mut queue = ret.sendq.write().await;
            let frames = queue.flush(monotonic_millis(), addr);
            #[cfg(feature = "recovery-policy")]
            ret.maintenance.publish(&queue, true);
            frames
        };
        if let Err(error) = Self::transmit_frames(&s, frames, addr, false, 0).await {
            if !matches!(error, RaknetError::SocketError) {
                return Err(error);
            }
            // A transient UDP send failure leaves the reliable request in flight.
            raknet_log_debug!("initial connected request will retry: {}", error);
        }
        raknet_log_debug!("waiting for handshake");
        ret.wait_for_handshake().await?;

        Ok(ret)
    }

    fn start_receiver(
        &self,
        s: &Arc<UdpSocket>,
        receiver: impl Into<DatagramInput>,
        user_data_sender: Sender<Vec<u8>>,
    ) {
        let mut receiver = receiver.into();
        let connected = self.close_notifier.clone();
        let peer_addr = self.peer_addr;
        let local_addr = self.local_addr;
        let sendq = self.sendq.clone();
        let recvq = self.recvq.clone();
        let last_heartbeat_time = self.last_heartbeat_time.clone();
        let handshake_complete = self.handshake_complete.clone();
        let send_capacity = self.send_capacity.clone();
        let maintenance = self.maintenance.clone();
        let s = s.clone();
        let enable_loss = self.enable_loss.clone();
        let loss_rate = self.loss_rate.clone();
        tokio::spawn(async move {
            let mut bulk_sender = None;
            // Application backpressure must not prevent ACK/NACK processing.
            let mut pending_data = std::collections::VecDeque::<Vec<u8>>::new();
            let mut pending_bytes = 0usize;
            let mut received_since_ack = 0usize;
            let mut decoded_frames = Vec::new();
            let mut ready_frames = Vec::new();
            let mut acks = Vec::new();
            let mut ack_packet = Vec::new();
            loop {
                // Coalesce ACKs only while datagrams are already queued. Never
                // wait for a timer or another datagram to acknowledge accepted data.
                if received_since_ack != 0
                    && (receiver.should_flush_ack() || received_since_ack >= 32)
                {
                    recvq.lock().await.take_ack_into(&mut acks);
                    if let Err(error) = Self::send_ack_ranges(
                        &s,
                        &acks,
                        &mut ack_packet,
                        &peer_addr,
                        enable_loss.load(Ordering::Relaxed),
                        loss_rate.load(Ordering::Relaxed),
                    )
                    .await
                    {
                        raknet_log_debug!("failed to send ACK: {}", error);
                    }
                    received_since_ack = 0;
                }
                let received = if pending_data.is_empty() {
                    tokio::select! {
                        _ = connected.acquire() => break,
                        message = receiver.recv() => message,
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
                        message = receiver.recv() => message,
                    }
                };
                match received {
                    Ok(true) => {}
                    Ok(false) => {
                        connected.close();
                        break;
                    }
                    Err(error) => {
                        raknet_log_debug!("datagram receive failed: {}", error);
                        connected.close();
                        break;
                    }
                }
                let buf = receiver.packet();
                let Some(&packet_id) = buf.first() else {
                    continue;
                };

                if received_since_ack != 0 {
                    received_since_ack += 1;
                }
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
                    debug_assert_eq!(ack.record_count as usize, ack.sequences.len());
                    let now = monotonic_millis();
                    let outgoing_frames = {
                        let mut sendq = sendq.write().await;
                        sendq.ack_ranges(&ack.sequences, now);
                        #[cfg(feature = "recovery-policy")]
                        let now = monotonic_millis();
                        let frames = sendq.flush(now, &peer_addr);
                        #[cfg(feature = "recovery-policy")]
                        maintenance.publish(&sendq, true);
                        frames
                    };
                    send_capacity.notify_waiters();
                    if let Err(error) = RaknetSocket::transmit_replies(
                        &s,
                        outgoing_frames,
                        &peer_addr,
                        &connected,
                        &mut bulk_sender,
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
                    debug_assert_eq!(nack.record_count as usize, nack.sequences.len());
                    let now = monotonic_millis();
                    let outgoing_frames = {
                        let mut sendq = sendq.write().await;
                        sendq.nack_ranges(&nack.sequences, now);
                        #[cfg(feature = "recovery-policy")]
                        let now = monotonic_millis();
                        let frames = sendq.flush(now, &peer_addr);
                        #[cfg(feature = "recovery-policy")]
                        maintenance.publish(&sendq, true);
                        frames
                    };
                    if let Err(error) = RaknetSocket::transmit_replies(
                        &s,
                        outgoing_frames,
                        &peer_addr,
                        &connected,
                        &mut bulk_sender,
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

                if let Err(error) = receiver.decode_into(&mut decoded_frames) {
                    raknet_log_debug!("ignoring malformed frame set: {}", error);
                    continue;
                }
                if decoded_frames.iter().any(FrameSetPacket::is_fragment) {
                    receiver.enable_fragment_worker(connected.clone());
                }
                if received_since_ack == 0 {
                    received_since_ack = 1;
                }
                ready_frames.clear();
                let invalid = {
                    let mut recvq = recvq.lock().await;
                    let mut invalid = false;
                    for frame in decoded_frames.drain(..) {
                        if let Err(error) = recvq.insert(frame) {
                            raknet_log_debug!(
                                "receive window or reassembly rejected frame: {}",
                                error
                            );
                            invalid = true;
                            break;
                        }
                        recvq.flush_into(&mut ready_frames);
                    }
                    if receiver.should_flush_ack() || received_since_ack >= 32 {
                        received_since_ack = 0;
                        recvq.take_ack_into(&mut acks);
                    } else {
                        acks.clear();
                    }
                    if recvq.needs_maintenance() && maintenance.idle.swap(false, Ordering::AcqRel) {
                        maintenance.wakeup.notify_one();
                    }
                    invalid
                };

                if invalid {
                    connected.close();
                    break;
                }
                let mut should_close = false;
                for frame in ready_frames.drain(..) {
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
                    )
                        => result,
                    };
                    match result {
                        Ok(
                            action @ (ControlAction::Continue | ControlAction::HandshakeComplete),
                        ) => {
                            // Flush control replies before exposing handshake readiness.
                            // Reliable frames remain queued until acknowledged.
                            let frames = {
                                let mut queue = sendq.write().await;
                                let frames = queue.flush(monotonic_millis(), &peer_addr);
                                #[cfg(feature = "recovery-policy")]
                                maintenance.publish(&queue, true);
                                frames
                            };
                            if !frames.is_empty() && maintenance.idle.swap(false, Ordering::AcqRel)
                            {
                                maintenance.wakeup.notify_one();
                            }
                            if let Err(error) = Self::transmit_frames(
                                &s,
                                frames,
                                &peer_addr,
                                enable_loss.load(Ordering::Relaxed),
                                loss_rate.load(Ordering::Relaxed),
                            )
                            .await
                            {
                                raknet_log_debug!("failed to send control reply: {}", error);
                                if !matches!(error, RaknetError::SocketError) {
                                    connected.close();
                                    should_close = true;
                                    break;
                                }
                            }
                            if matches!(action, ControlAction::HandshakeComplete) {
                                raknet_log_debug!("handshake complete");
                                handshake_complete.add_permits(1);
                            }
                        }
                        Ok(ControlAction::Disconnect) => {
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

                if let Err(error) = Self::send_ack_ranges(
                    &s,
                    &acks,
                    &mut ack_packet,
                    &peer_addr,
                    enable_loss.load(Ordering::Relaxed),
                    loss_rate.load(Ordering::Relaxed),
                )
                .await
                {
                    raknet_log_debug!("failed to send ACK: {}", error);
                }

                if should_close {
                    break;
                }
            }

            raknet_log_debug!("{} receive worker closed", peer_addr);
        });
    }

    #[cfg(feature = "recovery-policy")]
    async fn transmit_maintenance_frames(
        socket: &UdpSocket,
        frames: OutgoingFrames,
        peer: &SocketAddr,
        enable_loss: bool,
        loss_rate: u8,
    ) {
        // A probe must preserve its complete datagram. Splitting members with
        // the same sequence ID lets an ACK retire members lost independently.
        if frames.coalesced {
            if let Err(error) =
                Self::transmit_frames(socket, frames, peer, enable_loss, loss_rate).await
            {
                raknet_log_error!("failed to send maintenance datagram: {}", error);
            }
            return;
        }
        for frame in &frames {
            if let Err(error) = Self::send_frame(socket, frame, peer, enable_loss, loss_rate).await
            {
                raknet_log_error!("failed to send frame: {}", error);
            }
        }
    }

    fn start_tick(&self, s: &Arc<UdpSocket>, collecter: Option<Arc<Mutex<Sender<SocketAddr>>>>) {
        let connected = self.close_notifier.clone();
        let s = s.clone();
        let peer_addr = self.peer_addr;
        let sendq = self.sendq.clone();
        let recvq = self.recvq.clone();
        let maintenance = self.maintenance.clone();
        let mut last_monitor_tick = monotonic_millis();
        let enable_loss = self.enable_loss.clone();
        let loss_rate = self.loss_rate.clone();
        let last_heartbeat_time = self.last_heartbeat_time.clone();
        tokio::spawn(async move {
            let mut nacks = Vec::new();
            let mut nack_packet = Vec::new();
            let mut idle = false;
            loop {
                let delay = if idle {
                    (RECEIVE_TIMEOUT
                        - (monotonic_millis() - last_heartbeat_time.load(Ordering::Relaxed)))
                    .max(1) as u64
                } else {
                    SendQ::DEFAULT_TIMEOUT_MILLS as u64
                };
                #[cfg(feature = "recovery-policy")]
                {
                    let notified = maintenance.wakeup.notified();
                    tokio::pin!(notified);
                    let mut delay = delay;
                    if maintenance.deadline_driven.load(Ordering::Relaxed) {
                        notified.as_mut().enable();
                        let now = monotonic_millis();
                        let remaining = maintenance
                            .deadline
                            .load(Ordering::Acquire)
                            .saturating_sub(now)
                            .max(1) as u64;
                        delay = delay.min(remaining);
                        maintenance
                            .wake_at
                            .store(now.saturating_add(delay as i64), Ordering::Release);
                        // Recheck after arming: a producer may have compared its
                        // deadline against the previous, earlier timer while it was
                        // being replaced. Later producers notify the armed waiter.
                        let remaining = maintenance
                            .deadline
                            .load(Ordering::Acquire)
                            .saturating_sub(now)
                            .max(1) as u64;
                        delay = delay.min(remaining);
                        maintenance
                            .wake_at
                            .store(now.saturating_add(delay as i64), Ordering::Release);
                    }
                    tokio::select! {
                        _ = sleep(std::time::Duration::from_millis(delay)) => {},
                        _ = &mut notified => {},
                        _ = connected.acquire() => {},
                    }
                }
                #[cfg(not(feature = "recovery-policy"))]
                tokio::select! {
                    _ = sleep(std::time::Duration::from_millis(delay)) => {},
                    _ = maintenance.wakeup.notified(), if idle => {},
                    _ = connected.acquire() => {},
                }
                // Arm before the existing queue checks. A producer racing with
                // an idle decision leaves a Notify permit for the next wait.
                // A briefly empty queue does not make a busy connection idle.
                // Keep the periodic schedule until traffic has been quiet for
                // half a second, avoiding repeated sleep/wake transitions.
                let quiet = maintenance.enabled.load(Ordering::Relaxed)
                    && monotonic_millis() - last_heartbeat_time.load(Ordering::Relaxed)
                        >= MAINTENANCE_IDLE_MILLIS;
                if quiet {
                    maintenance.idle.store(true, Ordering::Release);
                }

                // Send NACK ranges.
                let receive_idle = {
                    let mut recvq = recvq.lock().await;
                    if recvq.fragments_expired() {
                        connected.close();
                        break;
                    }
                    recvq.take_nack_into(&mut nacks);
                    !recvq.needs_maintenance()
                };
                if !nacks.is_empty() {
                    match write_control_ranges_into(PacketID::Nack, &nacks, &mut nack_packet) {
                        Ok(()) => {
                            if let Err(error) = RaknetSocket::sendto(
                                &s,
                                &nack_packet,
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

                // Compute idleness while holding the locks already needed for
                // maintenance, rather than adding two more locks per active tick.
                let (outgoing_frames, send_idle) = {
                    let mut sendq = sendq.write().await;
                    let frames = sendq.flush(monotonic_millis(), &peer_addr);
                    #[cfg(feature = "recovery-policy")]
                    maintenance.publish(&sendq, false);
                    (frames, sendq.is_empty())
                };
                idle = quiet && receive_idle && send_idle;
                if !idle {
                    maintenance.idle.store(false, Ordering::Release);
                }
                #[cfg(feature = "recovery-policy")]
                Self::transmit_maintenance_frames(
                    &s,
                    outgoing_frames,
                    &peer_addr,
                    enable_loss.load(Ordering::Relaxed),
                    loss_rate.load(Ordering::Relaxed),
                )
                .await;
                #[cfg(not(feature = "recovery-policy"))]
                for frame in &outgoing_frames {
                    if let Err(error) = Self::send_frame(
                        &s,
                        frame,
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
                    last_monitor_tick = monotonic_millis();
                    if crate::log::ENABLE_RAKNET_LOG.load(Ordering::Relaxed) & 1 != 0 {
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
                    }
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
    /// 65,536 fragments or the 64 MiB send budget (including fragment overhead) return
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
        self.send_payload(buf, reliability, order_channel, None)
            .await
    }

    /// Send an owned, immutable payload without copying it into the retry queue.
    pub async fn send_bytes(&self, data: bytes::Bytes, reliability: Reliability) -> Result<()> {
        self.send_bytes_with_order_channel(data, reliability, 0)
            .await
    }

    /// Send an owned payload on a RakNet ordering channel.
    pub async fn send_bytes_with_order_channel(
        &self,
        data: bytes::Bytes,
        reliability: Reliability,
        order_channel: u8,
    ) -> Result<()> {
        self.send_payload(&data, reliability, order_channel, Some(&data))
            .await
    }

    /// Send an already available batch without waiting to collect more messages.
    ///
    /// Small ReliableOrdered messages can share standard RakNet frame sets up
    /// to the negotiated MTU. Loss feedback or a retransmission timeout restores
    /// individual sends for this connection. Other modes retain individual
    /// datagrams. A batch is validated before sending any member.
    /// Cancellation can send a prefix;
    /// already queued reliable messages continue delivery. Empty batches are a no-op.
    pub async fn send_batch(&self, messages: &[&[u8]], reliability: Reliability) -> Result<()> {
        self.send_batch_with_order_channel(messages, reliability, 0)
            .await
    }

    /// Send a borrowed batch on one ordering channel.
    pub async fn send_batch_with_order_channel(
        &self,
        messages: &[&[u8]],
        reliability: Reliability,
        order_channel: u8,
    ) -> Result<()> {
        self.send_batch_payload(messages, &[], reliability, order_channel)
            .await
    }

    /// Send owned buffers without copying their payload into the retry queue.
    pub async fn send_bytes_batch(
        &self,
        messages: &[bytes::Bytes],
        reliability: Reliability,
    ) -> Result<()> {
        self.send_bytes_batch_with_order_channel(messages, reliability, 0)
            .await
    }

    /// Send an owned batch on one ordering channel.
    pub async fn send_bytes_batch_with_order_channel(
        &self,
        messages: &[bytes::Bytes],
        reliability: Reliability,
        order_channel: u8,
    ) -> Result<()> {
        self.send_batch_payload(&[], messages, reliability, order_channel)
            .await
    }

    async fn send_batch_payload(
        &self,
        borrowed: &[&[u8]],
        owned: &[bytes::Bytes],
        reliability: Reliability,
        order_channel: u8,
    ) -> Result<()> {
        let count = borrowed.len() + owned.len();
        if count == 0 {
            return Ok(());
        }
        if count == 1 {
            return match owned.first() {
                Some(message) => {
                    self.send_payload(message, reliability, order_channel, Some(message))
                        .await
                }
                None => {
                    self.send_payload(borrowed[0], reliability, order_channel, None)
                        .await
                }
            };
        }
        let lengths = || {
            borrowed
                .iter()
                .map(|message| message.len())
                .chain(owned.iter().map(bytes::Bytes::len))
        };
        if borrowed
            .iter()
            .any(|message| message.first() != Some(&0xfe))
            || owned.iter().any(|message| message.first() != Some(&0xfe))
        {
            return Err(RaknetError::PacketHeaderError);
        }
        // Validate every member before reserving memory or assigning indexes.
        #[cfg(feature = "send-policy")]
        let can_coalesce = {
            let queue = self.sendq.read().await;
            if queue.send_options().is_some() {
                queue.batch_can_coalesce(reliability, lengths())?
            } else {
                queue.has_batch_capacity(reliability, lengths())?;
                let mtu = usize::from(queue.mtu());
                for length in lengths() {
                    if reliability != Reliability::ReliableOrdered
                        && length > mtu.saturating_sub(60)
                    {
                        return Err(RaknetError::PacketSizeExceedMTU);
                    }
                }
                queue.allows_coalescing()
                    && reliability == Reliability::ReliableOrdered
                    && lengths().zip(lengths().skip(1)).any(|(a, b)| {
                        a <= mtu.saturating_sub(60)
                            && b <= mtu.saturating_sub(60)
                            && a.saturating_add(b).saturating_add(24) <= mtu.saturating_sub(28)
                    })
            }
        };
        #[cfg(not(feature = "send-policy"))]
        let can_coalesce = {
            let queue = self.sendq.read().await;
            queue.has_batch_capacity(reliability, lengths())?;
            let mtu = usize::from(queue.mtu());
            for length in lengths() {
                if reliability != Reliability::ReliableOrdered && length > mtu.saturating_sub(60) {
                    return Err(RaknetError::PacketSizeExceedMTU);
                }
            }
            queue.allows_coalescing()
                && reliability == Reliability::ReliableOrdered
                && lengths().zip(lengths().skip(1)).any(|(a, b)| {
                    a <= mtu.saturating_sub(60)
                        && b <= mtu.saturating_sub(60)
                        && a.saturating_add(b).saturating_add(24) <= mtu.saturating_sub(28)
                })
        };
        // Avoid changing the pacing of ordinary messages when batching cannot
        // reduce the datagram count. No extra task or timer is introduced.
        if !can_coalesce {
            for &message in borrowed {
                self.send_payload(message, reliability, order_channel, None)
                    .await?;
            }
            for message in owned {
                self.send_payload(message, reliability, order_channel, Some(message))
                    .await?;
            }
            return Ok(());
        }
        if self.close_notifier.is_closed() {
            return Err(RaknetError::ConnectionClosed);
        }
        if self.handshake_complete.available_permits() == 0 {
            self.wait_for_handshake().await?;
        }
        loop {
            let available = self.send_capacity.notified();
            tokio::pin!(available);
            available.as_mut().enable();
            let frames = {
                let mut queue = self.sendq.write().await;
                if queue.has_batch_capacity(reliability, lengths())? {
                    if reliability == Reliability::ReliableOrdered {
                        queue.enable_coalescing();
                    }
                    for &message in borrowed {
                        queue.insert_with_order_channel(reliability, message, order_channel)?;
                    }
                    for message in owned {
                        queue.insert_bytes(reliability, message, order_channel)?;
                    }
                    let frames = queue.flush(monotonic_millis(), &self.peer_addr);
                    #[cfg(feature = "recovery-policy")]
                    self.maintenance.publish(&queue, true);
                    Some(frames)
                } else {
                    None
                }
            };
            if let Some(frames) = frames {
                if self.maintenance.idle.swap(false, Ordering::AcqRel) {
                    self.maintenance.wakeup.notify_one();
                }
                let udp = self.udp.upgrade().ok_or(RaknetError::ConnectionClosed)?;
                return Self::transmit_frames(
                    &udp,
                    frames,
                    &self.peer_addr,
                    self.enable_loss.load(Ordering::Relaxed),
                    self.loss_rate.load(Ordering::Relaxed),
                )
                .await;
            }
            tokio::select! {
                _ = self.close_notifier.acquire() => return Err(RaknetError::ConnectionClosed),
                _ = &mut available => {}
            }
        }
    }

    async fn send_payload(
        &self,
        buf: &[u8],
        reliability: Reliability,
        order_channel: u8,
        owned: Option<&bytes::Bytes>,
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
                    match owned {
                        Some(data) => sendq.insert_bytes(reliability, data, order_channel)?,
                        None => sendq.insert_with_order_channel(reliability, buf, order_channel)?,
                    }
                    let frames = sendq.flush(monotonic_millis(), &self.peer_addr);
                    #[cfg(feature = "recovery-policy")]
                    self.maintenance.publish(&sendq, true);
                    Some(frames)
                } else {
                    None
                }
            };
            if let Some(frames) = frames {
                if self.maintenance.idle.load(Ordering::Relaxed)
                    && self.maintenance.idle.swap(false, Ordering::AcqRel)
                {
                    self.maintenance.wakeup.notify_one();
                }
                // Send from the caller task to avoid an extra channel hop on
                // the application path. Reliable frames remain in the send queue
                // until acknowledged, including when this future is cancelled.
                let udp = self.udp.upgrade().ok_or(RaknetError::ConnectionClosed)?;
                let enable_loss = self.enable_loss.load(Ordering::Relaxed);
                let loss_rate = self.loss_rate.load(Ordering::Relaxed);
                return Self::transmit_frames(
                    &udp,
                    frames,
                    &self.peer_addr,
                    enable_loss,
                    loss_rate,
                )
                .await;
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
            // Register before checking the queue so an intervening ACK wakes
            // every concurrent flush caller, including an unpolled waiter.
            let acknowledged = self.send_capacity.notified();
            tokio::pin!(acknowledged);
            acknowledged.as_mut().enable();
            if self.close_notifier.is_closed() {
                return Err(RaknetError::ConnectionClosed);
            }
            if self.sendq.read().await.is_empty() {
                return Ok(());
            }
            tokio::select! {
                _ = self.close_notifier.acquire() => return Err(RaknetError::ConnectionClosed),
                _ = &mut acknowledged => {}
            }
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

    /// Receive a game payload as an immutable buffer that can be forwarded with
    /// `send_bytes` without copying its application bytes.
    pub async fn recv_bytes(&self) -> Result<bytes::Bytes> {
        self.recv().await.map(bytes::Bytes::from)
    }

    /// Receive one message, then drain up to `limit` messages already available.
    /// Clears `output` and never waits to fill a batch. Work is capped at 64
    /// messages per call; a zero limit returns immediately.
    pub async fn recv_batch(&self, output: &mut Vec<Vec<u8>>, limit: usize) -> Result<usize> {
        self.recv_batch_map(output, limit, |message| message).await
    }

    /// Receive an immediately available batch for forwarding with send_bytes_batch.
    pub async fn recv_bytes_batch(
        &self,
        output: &mut Vec<bytes::Bytes>,
        limit: usize,
    ) -> Result<usize> {
        self.recv_batch_map(output, limit, bytes::Bytes::from).await
    }

    async fn recv_batch_map<T>(
        &self,
        output: &mut Vec<T>,
        limit: usize,
        map: impl Fn(Vec<u8>) -> T,
    ) -> Result<usize> {
        output.clear();
        if limit == 0 {
            return Ok(0);
        }
        let mut receiver = self.user_data_receiver.lock().await;
        let first = receiver.recv().await.ok_or_else(|| {
            if self.close_notifier.is_closed() {
                RaknetError::ConnectionClosed
            } else {
                RaknetError::SocketError
            }
        })?;
        output.push(map(first));
        while output.len() < limit.min(64) {
            match receiver.try_recv() {
                Ok(message) => output.push(map(message)),
                Err(_) => break,
            }
        }
        Ok(output.len())
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

#[cfg(test)]
mod ack_batch_tests {
    use super::*;

    #[tokio::test]
    async fn channel_acks_flush_when_the_last_ready_datagram_is_consumed() {
        let (sender, receiver) = channel(2);
        let mut input = DatagramInput::from(receiver);
        assert!(input.should_flush_ack());
        sender.send(vec![0xfe, 1]).await.unwrap();
        assert!(!input.should_flush_ack());
        assert!(input.recv().await.unwrap());
        assert_eq!(input.packet(), &[0xfe, 1]);
        assert!(input.should_flush_ack());
        sender.send(vec![0xfe, 2]).await.unwrap();
        sender.send(vec![0xfe, 3]).await.unwrap();
        assert!(input.recv().await.unwrap());
        assert!(!input.should_flush_ack());
        assert!(input.recv().await.unwrap());
        assert!(input.should_flush_ack());
    }

    #[tokio::test]
    async fn queued_frames_coalesce_acks_without_waiting_for_control_packets() {
        timeout(std::time::Duration::from_secs(2), async {
            let udp = Arc::new(UdpSocket::bind("127.0.0.1:0").await.unwrap());
            let peer = UdpSocket::bind("127.0.0.1:0").await.unwrap();
            let addr = peer.local_addr().unwrap();
            let (input, receiver) = channel(100);
            for index in 0..40 {
                let mut frame =
                    FrameSetPacket::new(Reliability::ReliableOrdered, vec![0xfe, index as u8]);
                frame.sequence_number = index;
                frame.reliable_frame_index = index;
                frame.ordered_frame_index = index;
                input.send(frame.serialize().unwrap()).await.unwrap();
            }
            input
                .send(
                    write_packet_ack(&Ack {
                        record_count: 1,
                        sequences: vec![(0, 0)],
                    })
                    .unwrap(),
                )
                .await
                .unwrap();
            let (collector, _collected) = channel(1);
            let socket = RaknetSocket::from(
                &addr,
                &udp,
                receiver,
                1400,
                Arc::new(Mutex::new(collector)),
                11,
            )
            .await;
            let mut acknowledged = [false; 40];
            let mut ack_packets = 0;
            let mut buffer = [0; 2048];
            while acknowledged.iter().any(|value| !value) {
                let (size, _) = peer.recv_from(&mut buffer).await.unwrap();
                if buffer[0] != PacketID::Ack.to_u8() {
                    continue;
                }
                ack_packets += 1;
                for (start, end) in read_packet_ack(&buffer[..size]).unwrap().sequences {
                    for index in start..=end {
                        acknowledged[index as usize] = true;
                    }
                }
            }
            assert_eq!(ack_packets, 2);
            for index in 0..40 {
                assert_eq!(socket.recv().await.unwrap(), vec![0xfe, index]);
            }
            socket.close().await.unwrap();
        })
        .await
        .expect("accepted data was left unacknowledged");
    }
}

#[cfg(test)]
mod direct_input_tests {
    use super::*;

    #[tokio::test]
    async fn read_ahead_waits_for_readiness_without_delaying_empty_acks() {
        let socket = Arc::new(UdpSocket::bind("127.0.0.1:0").await.unwrap());
        let address = socket.local_addr().unwrap();
        let peer = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let mut input = DatagramInput::direct(socket, peer.local_addr().unwrap());
        // Clear cached readiness with an empty read before sending the packet.
        assert!(input.should_flush_ack());
        peer.send_to(&[0xfe, 42], address).await.unwrap();
        wait_for_read_ahead(&mut input).await;
        assert!(!input.should_flush_ack());
        assert!(input.packet().is_empty());
        assert!(input.recv().await.unwrap());
        assert_eq!(input.packet(), &[0xfe, 42]);
        assert!(input.should_flush_ack());
    }

    #[tokio::test]
    async fn direct_input_filters_foreign_peers_and_keeps_read_ahead_until_consumed() {
        timeout(std::time::Duration::from_secs(2), async {
            let socket = Arc::new(UdpSocket::bind("127.0.0.1:0").await.unwrap());
            let address = socket.local_addr().unwrap();
            let peer = UdpSocket::bind("127.0.0.1:0").await.unwrap();
            let foreign = UdpSocket::bind("127.0.0.1:0").await.unwrap();
            let mut input = DatagramInput::direct(socket, peer.local_addr().unwrap());
            foreign.send_to(&[0xfe, 99], address).await.unwrap();
            peer.send_to(&[0xfe, 1], address).await.unwrap();
            peer.send_to(&[0xfe, 2], address).await.unwrap();
            assert!(input.recv().await.unwrap());
            assert_eq!(input.packet(), &[0xfe, 1]);
            wait_for_read_ahead(&mut input).await;
            assert!(!input.should_flush_ack());
            assert!(!input.should_flush_ack());
            assert_eq!(input.packet(), &[0xfe, 1]);
            assert!(input.recv().await.unwrap());
            assert_eq!(input.packet(), &[0xfe, 2]);
            assert!(input.should_flush_ack());
        })
        .await
        .expect("direct datagram input stalled");
    }
}

#[cfg(test)]
mod vectored_send_tests {
    use super::*;

    #[tokio::test]
    async fn vectored_send_matches_serialized_wire_bytes() {
        let sender = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let receiver = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let address = receiver.local_addr().unwrap();
        for reliability in [
            Reliability::Unreliable,
            Reliability::UnreliableSequenced,
            Reliability::Reliable,
            Reliability::ReliableOrdered,
            Reliability::ReliableSequenced,
        ] {
            for size in [0, 64, 128, 129, 1300] {
                for fragmented in [false, true] {
                    let mut frame = FrameSetPacket::new(reliability, vec![0xfe; size]);
                    frame.sequence_number = 0x123456;
                    frame.reliable_frame_index = 0x654321;
                    frame.sequenced_frame_index = 0xabcdef;
                    frame.ordered_frame_index = 0x112233;
                    frame.order_channel = 31;
                    if fragmented {
                        frame.flags |= 16;
                        frame.compound_size = 3;
                        frame.compound_id = 12345;
                        frame.fragment_index = 2;
                    }
                    let expected = frame.serialize().unwrap();
                    RaknetSocket::send_frame(&sender, &frame, &address, false, 0)
                        .await
                        .unwrap();
                    let mut packet = [0; 2048];
                    let (length, _) = timeout(
                        std::time::Duration::from_secs(1),
                        receiver.recv_from(&mut packet),
                    )
                    .await
                    .unwrap()
                    .unwrap();
                    assert_eq!(&packet[..length], expected);
                    assert!(
                        matches!(receiver.try_recv_from(&mut packet), Err(error) if error.kind() == std::io::ErrorKind::WouldBlock)
                    );
                }
            }
        }
    }
}

#[cfg(test)]
mod tuning_tests {
    use super::*;
    use crate::RaknetListener;

    #[tokio::test]
    async fn fragment_worker_preserves_prefetch_filters_peers_and_stops_on_close() {
        timeout(std::time::Duration::from_secs(3), async {
            let socket = Arc::new(UdpSocket::bind("127.0.0.1:0").await.unwrap());
            let peer = UdpSocket::bind("127.0.0.1:0").await.unwrap();
            let foreign = UdpSocket::bind("127.0.0.1:0").await.unwrap();
            let address = socket.local_addr().unwrap();
            let mut input = DatagramInput::direct(socket, peer.local_addr().unwrap());
            peer.send_to(&[0xfe, 1], address).await.unwrap();
            assert!(input.recv().await.unwrap());
            assert_eq!(input.packet(), &[0xfe, 1]);
            peer.send_to(&[0xfe, 2], address).await.unwrap();
            wait_for_read_ahead(&mut input).await;
            assert!(!input.should_flush_ack());
            let close = Arc::new(tokio::sync::Semaphore::new(0));
            input.enable_fragment_worker(close.clone());
            input.enable_fragment_worker(close.clone());
            foreign.send_to(&[0xfe, 99], address).await.unwrap();
            peer.send_to(&[0xfe, 3], address).await.unwrap();
            for expected in [2, 3] {
                assert!(input.recv().await.unwrap());
                assert_eq!(input.packet(), &[0xfe, expected]);
            }
            close.close();
            assert!(!input.recv().await.unwrap());
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn small_fragment_reply_batches_start_the_bounded_sender() {
        timeout(std::time::Duration::from_secs(3), async {
            let socket = Arc::new(UdpSocket::bind("127.0.0.1:0").await.unwrap());
            let peer = UdpSocket::bind("127.0.0.1:0").await.unwrap();
            let address = peer.local_addr().unwrap();
            let mut queue = SendQ::new(1400);
            queue
                .insert(Reliability::ReliableOrdered, &[0xfe; 4096])
                .unwrap();
            let frames = queue.flush(0, &address);
            assert_eq!(frames.len(), 4);
            let expected: Vec<_> = frames
                .iter()
                .map(|frame| frame.serialize().unwrap())
                .collect();
            let close = Arc::new(tokio::sync::Semaphore::new(0));
            let mut sender = None;
            RaknetSocket::transmit_replies(
                &socket,
                frames,
                &address,
                &close,
                &mut sender,
                false,
                0,
            )
            .await
            .unwrap();
            assert!(sender.is_some());
            let mut packet = [0; 2048];
            for expected in expected {
                let (length, _) = peer.recv_from(&mut packet).await.unwrap();
                assert_eq!(&packet[..length], expected);
            }
            close.close();
            sender.unwrap().closed().await;
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn mtu_negotiation_clamps_to_both_peers_and_preserves_fragmented_echoes() {
        timeout(std::time::Duration::from_secs(5), async {
            for version in [10, 11] {
                for (maximum, requested, expected) in
                    [(1428, 1492, 1428), (1492, 576, 576), (1400, 1492, 1400)]
                {
                    let mut listener = RaknetListener::bind_with_maximum_mtu(
                        &"127.0.0.1:0".parse().unwrap(),
                        maximum,
                    )
                    .await
                    .unwrap();
                    listener.listen().await;
                    let client = RaknetSocket::connect_with_version_and_mtu(
                        &listener.local_addr().unwrap(),
                        version,
                        requested,
                    )
                    .await
                    .unwrap();
                    let server = listener.accept().await.unwrap();
                    assert_eq!(client.mtu(), expected);
                    assert_eq!(server.mtu(), expected);
                    let payload = vec![0xfe; 4096];
                    client
                        .send(&payload, Reliability::ReliableOrdered)
                        .await
                        .unwrap();
                    assert_eq!(server.recv().await.unwrap(), payload);
                    server
                        .send(&payload, Reliability::ReliableOrdered)
                        .await
                        .unwrap();
                    assert_eq!(client.recv().await.unwrap(), payload);
                    client.close().await.unwrap();
                    server.close().await.unwrap();
                    listener.close().await.unwrap();
                }
            }
        })
        .await
        .expect("MTU negotiation stalled");
    }

    #[tokio::test]
    async fn invalid_mtu_values_fail_before_binding_or_connecting() {
        let address = "127.0.0.1:0".parse().unwrap();
        for mtu in [0, 60, 1493, u16::MAX] {
            assert!(matches!(
                RaknetSocket::connect_with_version_and_mtu(&address, 11, mtu).await,
                Err(RaknetError::PacketSizeExceedMTU)
            ));
            assert!(matches!(
                RaknetListener::bind_with_maximum_mtu(&address, mtu).await,
                Err(RaknetError::PacketSizeExceedMTU)
            ));
        }
    }

    #[tokio::test]
    async fn bulk_dispatch_is_lazy_preserves_wire_bytes_and_stops_on_close() {
        timeout(std::time::Duration::from_secs(2), async {
            let socket = Arc::new(UdpSocket::bind("127.0.0.1:0").await.unwrap());
            let peer = UdpSocket::bind("127.0.0.1:0").await.unwrap();
            let address = peer.local_addr().unwrap();
            let close = Arc::new(tokio::sync::Semaphore::new(0));
            let mut sender = None;
            let mut frames = OutgoingFrames::new();
            frames.push(FrameSetPacket::new(
                Reliability::ReliableOrdered,
                vec![0xfe; 800],
            ));
            let expected = frames[0].serialize().unwrap();
            RaknetSocket::transmit_replies(
                &socket,
                frames,
                &address,
                &close,
                &mut sender,
                false,
                0,
            )
            .await
            .unwrap();
            assert!(sender.is_none());
            let mut packet = [0; 2048];
            let (length, _) = peer.recv_from(&mut packet).await.unwrap();
            assert_eq!(&packet[..length], expected);
            let mut frames = OutgoingFrames::new();
            let mut expected = Vec::new();
            for index in 1..=32 {
                let mut frame = FrameSetPacket::new(Reliability::ReliableOrdered, vec![0xfe; 800]);
                frame.sequence_number = index;
                expected.push(frame.serialize().unwrap());
                frames.push(frame);
            }
            RaknetSocket::transmit_replies(
                &socket,
                frames,
                &address,
                &close,
                &mut sender,
                false,
                0,
            )
            .await
            .unwrap();
            for expected in expected {
                let (length, _) = peer.recv_from(&mut packet).await.unwrap();
                assert_eq!(&packet[..length], expected);
            }
            close.close();
            sender.unwrap().closed().await;
        })
        .await
        .expect("bulk sender did not stop");
    }
}

#[cfg(test)]
mod maintenance_tests {
    use super::*;
    use crate::RaknetListener;

    async fn wait_for_idle(socket: &RaknetSocket) {
        timeout(std::time::Duration::from_secs(2), async {
            while !socket.maintenance.idle.load(Ordering::Acquire) {
                sleep(std::time::Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("quiet connection did not enter idle maintenance");
    }

    #[tokio::test]
    async fn a_lost_control_reply_wakes_an_idle_connection_for_retries() {
        timeout(std::time::Duration::from_secs(3), async {
            let udp = Arc::new(UdpSocket::bind("127.0.0.1:0").await.unwrap());
            let peer = UdpSocket::bind("127.0.0.1:0").await.unwrap();
            let (input, receiver) = channel(1);
            let (collector, _collected) = channel(1);
            let mut socket = RaknetSocket::from(
                &peer.local_addr().unwrap(),
                &udp,
                receiver,
                1400,
                Arc::new(Mutex::new(collector)),
                11,
            )
            .await;
            assert!(!socket.maintenance.enabled.load(Ordering::Acquire));
            socket.set_idle_maintenance(true);
            wait_for_idle(&socket).await;
            assert!(socket.maintenance.idle.load(Ordering::Acquire));
            socket.set_loss_rate(10);
            let request = ConnectionRequest {
                guid: 42,
                time: cur_timestamp_millis(),
                use_encryption: 0,
            };
            let frame = FrameSetPacket::new(
                Reliability::ReliableOrdered,
                write_packet_connection_request(&request).unwrap(),
            );
            input.send(frame.serialize().unwrap()).await.unwrap();
            while socket.sendq.read().await.get_sent_queue_size() == 0 {
                tokio::task::yield_now().await;
            }
            // The first reply and its ACK are deliberately lost. Recovery must
            // come from the maintenance task, without another incoming packet.
            sleep(std::time::Duration::from_millis(20)).await;
            let mut packet = [0; 2048];
            assert_eq!(
                peer.try_recv_from(&mut packet).unwrap_err().kind(),
                std::io::ErrorKind::WouldBlock
            );
            socket.set_loss_rate(0);
            let (length, _) = timeout(
                std::time::Duration::from_secs(1),
                peer.recv_from(&mut packet),
            )
            .await
            .unwrap()
            .unwrap();
            let (reply, _) = FrameSetPacket::deserialize(&packet[..length]).unwrap();
            assert_eq!(reply.data[0], PacketID::ConnectionRequestAccepted.to_u8());
            read_packet_connection_request_accepted(&reply.data).unwrap();
            socket.close().await.unwrap();
        })
        .await
        .expect("lost control reply stalled idle maintenance");
    }

    #[tokio::test]
    async fn idle_connections_wake_for_owned_sends_and_loss_recovery() {
        timeout(std::time::Duration::from_secs(3), async {
            let mut listener = RaknetListener::bind(&"127.0.0.1:0".parse().unwrap())
                .await
                .unwrap()
                .with_idle_maintenance(true);
            listener.listen().await;
            let mut client = RaknetSocket::connect(&listener.local_addr().unwrap())
                .await
                .unwrap();
            let server = listener.accept().await.unwrap();
            assert!(!client.maintenance.enabled.load(Ordering::Acquire));
            assert!(server.maintenance.enabled.load(Ordering::Acquire));
            client.set_idle_maintenance(true);
            client.flush().await.unwrap();
            server.flush().await.unwrap();
            wait_for_idle(&client).await;
            assert!(client.maintenance.idle.load(Ordering::Acquire));
            client.set_loss_rate(10);
            let payload = bytes::Bytes::from(vec![0xfe; 4096]);
            client
                .send_bytes_with_order_channel(payload.clone(), Reliability::ReliableOrdered, 7)
                .await
                .unwrap();
            sleep(std::time::Duration::from_millis(10)).await;
            client.set_loss_rate(0);
            assert_eq!(
                timeout(std::time::Duration::from_millis(500), server.recv_bytes())
                    .await
                    .unwrap()
                    .unwrap(),
                payload
            );
            client.flush().await.unwrap();
            server
                .send_bytes(payload.clone(), Reliability::ReliableOrdered)
                .await
                .unwrap();
            assert_eq!(client.recv_bytes().await.unwrap(), payload);
            client.set_idle_maintenance(false);
            server.set_idle_maintenance(false);
            sleep(std::time::Duration::from_millis(100)).await;
            assert!(!client.maintenance.enabled.load(Ordering::Acquire));
            assert!(!client.maintenance.idle.load(Ordering::Acquire));
            assert!(!server.maintenance.idle.load(Ordering::Acquire));
            listener.close().await.unwrap();
        })
        .await
        .expect("idle maintenance or owned send stalled");
    }
}

#[cfg(feature = "send-policy")]
impl RaknetSocket {
    /// Configure this connection's reliable flight window and queue budgets.
    /// Existing messages remain queued; lowering limits does not discard data.
    /// Apply before starting bulk traffic to establish the unreliable reserve.
    pub async fn set_send_options(&self, options: crate::SendOptions) -> Result<()> {
        if self.close_notifier.is_closed() {
            return Err(RaknetError::ConnectionClosed);
        }
        {
            let mut queue = self.sendq.write().await;
            queue.set_send_options(options)?;
            queue.register_capacity_notifier(&self.send_capacity);
        }
        self.send_capacity.notify_waiters();
        self.maintenance.idle.store(false, Ordering::Release);
        self.maintenance.wakeup.notify_one();
        Ok(())
    }

    /// Return configured limits, or `None` for the original queue policy.
    pub async fn send_options(&self) -> Option<crate::SendOptions> {
        self.sendq.read().await.send_options()
    }
}

#[cfg(feature = "recovery-policy")]
impl RaknetSocket {
    /// Configure optional early retransmission and ACK-progress backoff.
    /// Apply on both peers before traffic when evaluating recovery latency.
    /// This does not change the base RTO estimator or RakNet wire format.
    pub async fn set_recovery_options(&self, options: crate::RecoveryOptions) -> Result<()> {
        if self.close_notifier.is_closed() {
            return Err(RaknetError::ConnectionClosed);
        }
        let mut queue = self.sendq.write().await;
        queue.set_recovery_options(options, monotonic_millis());
        let deadlines = options.deadline_driven || options.tail_probe_min_delay.is_some();
        self.maintenance
            .deadline_driven
            .store(deadlines, Ordering::Release);
        if !deadlines {
            self.maintenance.deadline.store(i64::MAX, Ordering::Release);
        }

        #[cfg(feature = "recovery-policy")]
        self.maintenance.publish(&queue, true);
        self.maintenance.wakeup.notify_one();
        Ok(())
    }

    /// Return the connection's recovery configuration; default options disable probes.
    pub async fn recovery_options(&self) -> crate::RecoveryOptions {
        self.sendq.read().await.recovery_options()
    }
}

#[cfg(all(test, feature = "recovery-policy"))]
mod recovery_tests {
    use super::*;
    #[tokio::test]
    async fn maintenance_preserves_a_packed_probe_as_one_datagram() {
        let sender = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let receiver = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let address = receiver.local_addr().unwrap();
        let mut queue = SendQ::new(1400);
        queue.enable_coalescing();
        for value in 0..8 {
            queue
                .insert(Reliability::ReliableOrdered, &[0xfe, value])
                .unwrap();
        }
        let frames = queue.flush(0, &address);
        assert_eq!(frames.len(), 8);
        assert!(frames.coalesced);
        RaknetSocket::transmit_maintenance_frames(&sender, frames, &address, false, 0).await;
        let mut wire = [0; 1500];
        let (length, _) = timeout(
            std::time::Duration::from_secs(1),
            receiver.recv_from(&mut wire),
        )
        .await
        .unwrap()
        .unwrap();
        let mut decoded = Vec::new();
        FrameVec::decode_into(&wire[..length], &mut decoded).unwrap();
        assert_eq!(decoded.len(), 8);
        for (value, frame) in decoded.iter().enumerate() {
            assert_eq!(frame.data.as_ref(), &[0xfe, value as u8]);
            assert_eq!(frame.sequence_number, decoded[0].sequence_number);
        }
        assert!(
            timeout(
                std::time::Duration::from_millis(20),
                receiver.recv_from(&mut wire)
            )
            .await
            .is_err()
        );
    }

    #[tokio::test]
    async fn ack_lock_contention_does_not_backdate_new_frames() {
        timeout(std::time::Duration::from_secs(3), async {
            let udp = Arc::new(UdpSocket::bind("127.0.0.1:0").await.unwrap());
            let peer = UdpSocket::bind("127.0.0.1:0").await.unwrap();
            let address = peer.local_addr().unwrap();
            let (input, receiver) = channel(1);
            let (collector, _collected) = channel(1);
            let socket = RaknetSocket::from(
                &address,
                &udp,
                receiver,
                1400,
                Arc::new(Mutex::new(collector)),
                11,
            )
            .await;
            let mut queue = socket.sendq.write().await;
            // Keep maintenance retries outside the test while simulating contention.
            queue.insert(Reliability::ReliableOrdered, &[0xfe]).unwrap();
            queue.flush(0, &address);
            queue.ack(0, 100_000);
            for _ in 0..64 {
                queue
                    .insert(Reliability::ReliableOrdered, &[0xfe, 1])
                    .unwrap();
            }
            assert_eq!(queue.flush(monotonic_millis(), &address).len(), 64);
            queue
                .insert(Reliability::ReliableOrdered, &[0xfe, 2])
                .unwrap();
            let mut ack = Vec::new();
            write_control_ranges_into(PacketID::Ack, &[(1, 64)], &mut ack).unwrap();
            input.send(ack).await.unwrap();
            // On this single-thread runtime, capacity is released before the
            // ACK worker blocks on the held queue lock.
            let _capacity = input.reserve().await.unwrap();
            sleep(std::time::Duration::from_millis(20)).await;
            let released_at = monotonic_millis();
            drop(queue);
            let mut queue = socket.sendq.write().await;
            assert_eq!(queue.get_sent_queue_size(), 1);
            let deadline = released_at + queue.get_rto() - 1;
            assert!(
                queue.flush(deadline, &address).is_empty(),
                "new frames must use a timestamp taken after the queue lock"
            );
            drop(queue);
            socket.close().await.unwrap();
        })
        .await
        .expect("ACK lock contention test stalled");
    }

    #[tokio::test]
    async fn earlier_deadline_wakes_active_maintenance() {
        let maintenance = Maintenance::default();
        maintenance.deadline_driven.store(true, Ordering::Release);
        let mut queue = SendQ::new(1400);
        queue
            .insert(Reliability::ReliableOrdered, &[0xfe, 1])
            .unwrap();
        queue.flush(100, &"127.0.0.1:19132".parse().unwrap());
        #[cfg(feature = "recovery-policy")]
        maintenance.publish(&queue, true);
        assert_eq!(maintenance.deadline.load(Ordering::Acquire), 150);
        timeout(
            std::time::Duration::from_secs(1),
            maintenance.wakeup.notified(),
        )
        .await
        .unwrap();
        #[cfg(feature = "recovery-policy")]
        maintenance.publish(&queue, false);
        queue.ack(0, 101);
        queue.flush(101, &"127.0.0.1:19132".parse().unwrap());
        #[cfg(feature = "recovery-policy")]
        maintenance.publish(&queue, true);
        assert_eq!(maintenance.deadline.load(Ordering::Acquire), i64::MAX);
    }
}

#[cfg(all(test, feature = "recovery-policy"))]
mod recovery_timer_tests {
    use super::*;
    use std::{
        future::Future,
        task::{Context, Waker},
    };

    #[test]
    fn later_deadlines_do_not_interrupt_an_earlier_armed_timer() {
        let maintenance = Maintenance::default();
        maintenance.deadline_driven.store(true, Ordering::Release);
        maintenance.wake_at.store(140, Ordering::Release);
        let address = "127.0.0.1:19132".parse().unwrap();
        let mut queue = SendQ::new(1400);
        queue
            .insert(Reliability::ReliableOrdered, &[0xfe, 1])
            .unwrap();
        queue.flush(100, &address);
        #[cfg(feature = "recovery-policy")]
        maintenance.publish(&queue, true);
        let notified = maintenance.wakeup.notified();
        tokio::pin!(notified);
        let mut context = Context::from_waker(Waker::noop());
        assert!(notified.as_mut().poll(&mut context).is_pending());
        let mut earlier = SendQ::new(1400);
        earlier
            .insert(Reliability::ReliableOrdered, &[0xfe, 2])
            .unwrap();
        earlier.flush(80, &address);
        #[cfg(feature = "recovery-policy")]
        maintenance.publish(&earlier, true);
        assert!(notified.as_mut().poll(&mut context).is_ready());
    }
}
