use std::collections::HashMap;
use std::{net::SocketAddr, sync::Arc};
use tokio::net::UdpSocket;
use tokio::sync::mpsc::channel;
use tokio::sync::mpsc::{Receiver, Sender};
use tokio::sync::{Mutex, Notify, watch};

use crate::error::{RaknetError, Result};
use crate::packet::*;
use crate::utils::*;
use crate::{raknet_log_debug, raknet_log_error, socket::*};

const SERVER_NAME: &str = "Rust Raknet Server";
const MAX_CONNECTION: u32 = 99999;

struct SessionSender {
    sender: Sender<Vec<u8>>,
    close: Arc<tokio::sync::Semaphore>,
    guid: u64,
    mtu: u16,
}

// The authoritative session map handles handshakes and shutdown. This bounded,
// direct-mapped cache avoids hashing the common dispatch path. A slot collision
// is only a cache miss: the complete peer address is always checked.
struct SessionDispatchCache {
    entries: Vec<Option<CachedSession>>,
}

struct CachedSession {
    address: SocketAddr,
    sender: Sender<Vec<u8>>,
    close: Arc<tokio::sync::Semaphore>,
}

impl Default for SessionDispatchCache {
    fn default() -> Self {
        Self {
            entries: std::iter::repeat_with(|| None).take(Self::LIMIT).collect(),
        }
    }
}

impl SessionDispatchCache {
    const LIMIT: usize = 4096;

    fn slot(address: &SocketAddr) -> usize {
        usize::from(address.port()) % Self::LIMIT
    }

    fn sender(&self, address: &SocketAddr) -> Option<&Sender<Vec<u8>>> {
        self.entries[Self::slot(address)]
            .as_ref()
            .filter(|entry| {
                entry.address == *address && !entry.close.is_closed() && !entry.sender.is_closed()
            })
            .map(|entry| &entry.sender)
    }

    fn remember(&mut self, address: SocketAddr, session: &SessionSender) {
        self.entries[Self::slot(&address)] = Some(CachedSession {
            address,
            sender: session.sender.clone(),
            close: session.close.clone(),
        });
    }

    fn forget(&mut self, address: &SocketAddr) {
        let slot = &mut self.entries[Self::slot(address)];
        if slot.as_ref().is_some_and(|entry| entry.address == *address) {
            *slot = None;
        }
    }

    fn prune(&mut self) {
        for slot in &mut self.entries {
            if slot
                .as_ref()
                .is_some_and(|entry| entry.close.is_closed() || entry.sender.is_closed())
            {
                *slot = None;
            }
        }
    }
}

/// A RakNet UDP server that accepts incoming connections.
pub struct RaknetListener {
    motd: String,
    shards: Vec<Self>,
    socket: Option<Arc<UdpSocket>>,
    guid: u64,
    listened: bool,
    maximum_mtu: u16,
    connection_receiver: Receiver<RaknetSocket>,
    connection_sender: Sender<RaknetSocket>,
    sessions: Arc<Mutex<HashMap<SocketAddr, SessionSender>>>,
    close_notifier: Arc<tokio::sync::Semaphore>,
    all_session_closed_notifier: Arc<Notify>,
    version_map: Arc<Mutex<HashMap<SocketAddr, u8>>>,
    motd_receiver: watch::Receiver<String>,
    motd_sender: watch::Sender<String>,
}

impl RaknetListener {
    /// Bind a RakNet listener to the specified UDP address.
    ///
    /// Call [`listen`](Self::listen) before accepting connections.
    pub async fn bind(sockaddr: &SocketAddr) -> Result<Self> {
        let socket =
            std::net::UdpSocket::bind(sockaddr).map_err(|_| RaknetError::BindAddressError)?;
        // A shared listener receives bursts from every connection. This changes
        // only this socket; the OS may clamp the requested receive buffer.
        if let Err(error) = socket2::SockRef::from(&socket).set_recv_buffer_size(2 * 1024 * 1024) {
            raknet_log_debug!("could not enlarge the listener receive buffer: {error}");
        }
        Self::from_std(socket).await
    }

    /// Bind a listener with an explicit maximum nominal MTU in bytes.
    ///
    /// The maximum must be between 61 and 1,492 bytes. Offline negotiation
    /// chooses the smaller of this limit and the client's request. Ordinary
    /// listeners retain their 1,400-byte maximum.
    pub async fn bind_with_maximum_mtu(sockaddr: &SocketAddr, maximum_mtu: u16) -> Result<Self> {
        if !(61..=RAKNET_MAX_MTU).contains(&maximum_mtu) {
            return Err(RaknetError::PacketSizeExceedMTU);
        }
        let mut listener = Self::bind(sockaddr).await?;
        listener.maximum_mtu = maximum_mtu;
        Ok(listener)
    }

    /// Bind a fixed group of UDP receive sockets on Linux.
    ///
    /// All shards use the same address, server GUID, MOTD and accept backlog.
    /// Linux `SO_REUSEPORT` distributes peer flows across the fixed group. Create
    /// every shard before listening and retain the group for the listener's lifetime.
    /// The count must be at most 64; one shard uses the ordinary binding path.
    /// This changes only these sockets and does not alter system network limits.
    #[cfg(target_os = "linux")]
    pub async fn bind_with_socket_shards(
        sockaddr: &SocketAddr,
        count: std::num::NonZeroUsize,
    ) -> Result<Self> {
        if count.get() == 1 {
            return Self::bind(sockaddr).await;
        }
        if count.get() > 64 {
            return Err(RaknetError::SocketError);
        }
        fn bind_socket(address: &SocketAddr) -> Result<std::net::UdpSocket> {
            let socket = socket2::Socket::new(
                socket2::Domain::for_address(*address),
                socket2::Type::DGRAM,
                Some(socket2::Protocol::UDP),
            )
            .map_err(|_| RaknetError::BindAddressError)?;
            socket
                .set_reuse_port(true)
                .map_err(|_| RaknetError::SocketError)?;
            socket
                .set_recv_buffer_size(2 * 1024 * 1024)
                .map_err(|_| RaknetError::SocketError)?;
            socket
                .bind(&(*address).into())
                .map_err(|_| RaknetError::BindAddressError)?;
            Ok(socket.into())
        }
        let mut listener = Self::from_std(bind_socket(sockaddr)?).await?;
        let address = listener.local_addr()?;
        for _ in 1..count.get() {
            let mut shard = Self::from_std(bind_socket(&address)?).await?;
            shard.guid = listener.guid;
            shard.connection_sender = listener.connection_sender.clone();
            shard.motd_sender = listener.motd_sender.clone();
            shard.motd_receiver = listener.motd_receiver.clone();
            listener.shards.push(shard);
        }
        Ok(listener)
    }

    /// Bind with a requested per-socket UDP receive buffer size.
    ///
    /// The OS may clamp the size to its existing limits. No system limits are
    /// changed. Unlike [`bind`](Self::bind), a failed buffer request returns an error.
    /// [`from_std`](Self::from_std) preserves the supplied socket's buffer settings.
    pub async fn bind_with_receive_buffer_size(
        sockaddr: &SocketAddr,
        size: std::num::NonZeroUsize,
    ) -> Result<Self> {
        if size.get() > i32::MAX as usize {
            return Err(RaknetError::SocketError);
        }
        let socket =
            std::net::UdpSocket::bind(sockaddr).map_err(|_| RaknetError::BindAddressError)?;
        socket2::SockRef::from(&socket)
            .set_recv_buffer_size(size.get())
            .map_err(|_| RaknetError::SocketError)?;
        Self::from_std(socket).await
    }

    /// Sets the number of connections that may wait for [`accept`](Self::accept).
    ///
    /// The default backlog is 128. When it is full, new offline handshakes wait
    /// for a retried request instead of allocating and immediately closing a session.
    ///
    /// # Panics
    /// Panics if called after [`listen`](Self::listen).
    pub fn with_accept_backlog(mut self, backlog: std::num::NonZeroUsize) -> Self {
        assert!(
            !self.listened,
            "configure the accept backlog before listening"
        );
        let (sender, receiver) = channel(backlog.get());
        for shard in &mut self.shards {
            shard.connection_sender = sender.clone();
        }
        self.connection_sender = sender;
        self.connection_receiver = receiver;
        self
    }

    /// Creates a listener from a standard UDP socket.
    pub async fn from_std(socket: std::net::UdpSocket) -> Result<Self> {
        socket
            .set_nonblocking(true)
            .map_err(|_| RaknetError::SetRaknetRawSocketError)?;
        let socket =
            UdpSocket::from_std(socket).map_err(|_| RaknetError::SetRaknetRawSocketError)?;
        Self::from_udp_socket(socket).await
    }

    async fn from_udp_socket(socket: UdpSocket) -> Result<Self> {
        let (connection_sender, connection_receiver) = channel::<RaknetSocket>(128);
        let (motd_sender, motd_receiver) = watch::channel(String::new());
        let listener = Self {
            motd: String::new(),
            shards: Vec::new(),
            socket: Some(Arc::new(socket)),
            guid: rand::random(),
            listened: false,
            maximum_mtu: RAKNET_CLIENT_MTU,
            connection_receiver,
            connection_sender,
            sessions: Arc::new(Mutex::new(HashMap::new())),
            close_notifier: Arc::new(tokio::sync::Semaphore::new(0)),
            all_session_closed_notifier: Arc::new(Notify::new()),
            version_map: Arc::new(Mutex::new(HashMap::new())),
            motd_receiver,
            motd_sender,
        };

        Ok(listener)
    }

    async fn start_session_collect(
        &self,
        socket: &Arc<UdpSocket>,
        sessions: &Arc<Mutex<HashMap<SocketAddr, SessionSender>>>,
        mut collect_receiver: Receiver<SocketAddr>,
    ) {
        let sessions = sessions.clone();
        let version_map = self.version_map.clone();
        let socket = socket.clone();
        let close_notifier = self.close_notifier.clone();
        let all_session_closed_notifier = self.all_session_closed_notifier.clone();
        tokio::spawn(async move {
            loop {
                let addr: SocketAddr;

                tokio::select! {
                    a = collect_receiver.recv() => {
                        match a {
                            Some(p) => { addr = p },
                            None => {
                                raknet_log_debug!("session collector closed");
                                break;
                            },
                        };
                    },
                    _ = close_notifier.acquire() => {
                        raknet_log_debug!("session collector received close notification");
                        break;
                    }
                }

                let mut sessions = sessions.lock().await;
                // A late collector message from an old peer must not remove
                // a replacement connection that reused the same address.
                if sessions
                    .get(&addr)
                    .is_some_and(|session| session.close.is_closed())
                {
                    match socket.send_to(&[PacketID::Disconnect.to_u8()], addr).await {
                        Ok(_) => {}
                        Err(e) => {
                            raknet_log_error!("udp socket send_to error : {}", e);
                        }
                    };
                    sessions.remove(&addr);
                    version_map.lock().await.remove(&addr);
                    raknet_log_debug!("collect socket : {}", addr);
                }
            }

            let mut sessions = sessions.lock().await;

            for (addr, session) in sessions.iter() {
                session.close.close();

                match socket.send_to(&[PacketID::Disconnect.to_u8()], addr).await {
                    Ok(_) => {}
                    Err(e) => {
                        raknet_log_error!("udp socket send_to error : {}", e);
                    }
                };
            }

            while !sessions.is_empty() {
                let addr = match collect_receiver.recv().await {
                    Some(p) => p,
                    None => {
                        raknet_log_error!(
                            "failed to clean up sessions; some connections may still be open"
                        );
                        break;
                    }
                };

                if sessions.contains_key(&addr) {
                    match socket.send_to(&[PacketID::Disconnect.to_u8()], addr).await {
                        Ok(_) => {}
                        Err(e) => {
                            raknet_log_error!("udp socket send_to error : {}", e);
                        }
                    };
                    sessions.remove(&addr);
                    version_map.lock().await.remove(&addr);
                    raknet_log_debug!("collect socket : {}", addr);
                }
            }

            sessions.clear();
            version_map.lock().await.clear();
            all_session_closed_notifier.notify_one();

            raknet_log_debug!("session collect closed");
        });
    }

    /// Start receiving connection requests.
    ///
    /// Call this before [`accept`](Self::accept). Repeated calls have no effect.
    pub async fn listen(&mut self) {
        if self.close_notifier.is_closed() || self.listened {
            return;
        }
        for shard in &mut self.shards {
            // Only the parent owns discovery metadata. Prevent a shard from
            // replacing the shared MOTD with its default during startup.
            shard.motd = self.motd.clone();
        }
        self.listen_single().await;
        for shard in &mut self.shards {
            shard.motd = self.motd.clone();
            shard.listen_single().await;
        }
    }

    async fn listen_single(&mut self) {
        if self.close_notifier.is_closed() || self.listened {
            return;
        }

        let Some(socket) = self.socket.as_ref().cloned() else {
            return;
        };
        let local_addr = match socket.local_addr() {
            Ok(address) => address,
            Err(error) => {
                raknet_log_error!("failed to query listener address: {error}");
                return;
            }
        };

        if self.motd.is_empty() {
            let _ = self
                .set_motd(
                    SERVER_NAME,
                    MAX_CONNECTION,
                    "486",
                    "1.18.11",
                    "Survival",
                    local_addr.port(),
                )
                .await;
        }

        let guid = self.guid;
        let maximum_mtu = self.maximum_mtu;
        let sessions = self.sessions.clone();
        let connection_sender = self.connection_sender.clone();

        self.listened = true;

        let (collect_sender, collect_receiver) = channel::<SocketAddr>(10);
        let collect_sender = Arc::new(Mutex::new(collect_sender));
        self.start_session_collect(&socket, &sessions, collect_receiver)
            .await;

        let close_notify = self.close_notifier.clone();
        let version_map = self.version_map.clone();
        let mut motd_receiver = self.motd_receiver.clone();

        tokio::spawn(async move {
            let mut buf = [0u8; 2048];
            let mut pending_versions = HashMap::<SocketAddr, (u8, std::time::Instant)>::new();
            let mut dispatch_cache = SessionDispatchCache::default();
            let mut cleanup = tokio::time::interval(std::time::Duration::from_secs(30));

            raknet_log_debug!("start listen worker : {}", local_addr);

            loop {
                let size: usize;
                let addr: SocketAddr;

                tokio::select! {
                    a = socket.recv_from(&mut buf) => {
                        match a {
                            Ok(p) => {
                                size = p.0;
                                addr = p.1;
                            },
                            Err(e) => {
                                raknet_log_debug!("server recv_from error {}" , e);
                                break;
                            },
                        };
                    },
                    _ = cleanup.tick() => {
                        pending_versions.retain(|_, (_, time)| time.elapsed().as_secs() < 60);
                        dispatch_cache.prune();
                        continue;
                    },
                    changed = motd_receiver.changed() => {
                        if changed.is_err() {
                            break;
                        }
                        let _ = motd_receiver.borrow_and_update();
                        continue;
                    },
                    _ = close_notify.acquire() => {
                        raknet_log_debug!("listen close notified");
                        break;
                    }
                }

                if size == 0 {
                    continue;
                }

                let cur_status = match PacketID::from(buf[0]) {
                    Ok(p) => p,
                    Err(e) => {
                        raknet_log_debug!("parse packetid faild : {:?}", e);
                        continue;
                    }
                };

                match cur_status {
                    PacketID::UnconnectedPing1 => {
                        let _ping = match read_packet_ping(&buf[..size]) {
                            Ok(p) => p,
                            Err(_) => continue,
                        };

                        let packet = crate::packet::PacketUnconnectedPong {
                            time: cur_timestamp_millis(),
                            guid,
                            motd: motd_receiver.borrow_and_update().clone(),
                        };

                        let pong = match write_packet_pong(&packet) {
                            Ok(p) => p,
                            Err(_) => continue,
                        };

                        match socket.send_to(&pong, addr).await {
                            Ok(_) => {}
                            Err(e) => {
                                raknet_log_error!("udp socket send_to error : {}", e);
                            }
                        };
                        continue;
                    }
                    PacketID::UnconnectedPing2 => {
                        match read_packet_ping(&buf[..size]) {
                            Ok(p) => p,
                            Err(_) => continue,
                        };

                        let packet = crate::packet::PacketUnconnectedPong {
                            time: cur_timestamp_millis(),
                            guid,
                            motd: motd_receiver.borrow_and_update().clone(),
                        };

                        let pong = match write_packet_pong(&packet) {
                            Ok(p) => p,
                            Err(_) => continue,
                        };

                        match socket.send_to(&pong, addr).await {
                            Ok(_) => {}
                            Err(e) => {
                                raknet_log_error!("udp socket send_to error : {}", e);
                            }
                        };
                        continue;
                    }
                    PacketID::OpenConnectionRequest1 => {
                        let req = match read_packet_connection_open_request_1(&buf[..size]) {
                            Ok(p) => p,
                            Err(_) => continue,
                        };

                        if !RAKNET_PROTOCOL_VERSION_LIST
                            .as_slice()
                            .contains(&req.protocol_version)
                        {
                            let packet = crate::packet::IncompatibleProtocolVersion {
                                server_protocol: RAKNET_PROTOCOL_VERSION,
                                server_guid: guid,
                            };
                            let buf = match write_packet_incompatible_protocol_version(&packet) {
                                Ok(buf) => buf,
                                Err(error) => {
                                    raknet_log_error!("failed to encode protocol error: {error}");
                                    continue;
                                }
                            };

                            match socket.send_to(&buf, addr).await {
                                Ok(_) => {}
                                Err(e) => {
                                    raknet_log_error!("udp socket send_to error : {}", e);
                                }
                            };
                            continue;
                        }
                        if pending_versions.len() >= 4096 && !pending_versions.contains_key(&addr) {
                            continue;
                        }
                        pending_versions
                            .insert(addr, (req.protocol_version, std::time::Instant::now()));

                        let packet = crate::packet::OpenConnectionReply1 {
                            guid,
                            // Encryption is not negotiated by this implementation.
                            use_encryption: 0x00,
                            mtu_size: req.mtu_size.min(maximum_mtu),
                        };

                        let reply = match write_packet_connection_open_reply_1(&packet) {
                            Ok(p) => p,
                            Err(_) => continue,
                        };

                        match socket.send_to(&reply, addr).await {
                            Ok(_) => {}
                            Err(e) => {
                                raknet_log_error!("udp socket send_to error : {}", e);
                            }
                        };
                        continue;
                    }
                    PacketID::OpenConnectionRequest2 => {
                        dispatch_cache.forget(&addr);
                        let req = match read_packet_connection_open_request_2(&buf[..size]) {
                            Ok(p) => p,
                            Err(_) => continue,
                        };

                        if !(61..=maximum_mtu).contains(&req.mtu) {
                            continue;
                        }
                        let existing = sessions
                            .lock()
                            .await
                            .get(&addr)
                            .map(|session| (session.guid, session.mtu));
                        if let Some((client_guid, mtu)) = existing {
                            // A lost offline reply causes Request2 retransmission.
                            // Replay the negotiated response without creating a session.
                            let reply = if client_guid == req.guid {
                                write_packet_connection_open_reply_2(&OpenConnectionReply2 {
                                    guid,
                                    address: addr,
                                    mtu,
                                    encryption_enabled: 0,
                                })
                            } else {
                                write_packet_already_connected(&AlreadyConnected { guid })
                            };
                            if let Ok(reply) = reply {
                                let _ = socket.send_to(&reply, addr).await;
                            }
                            continue;
                        }

                        // Reserve acceptance before acknowledging the offline handshake.
                        // A full backlog must not create a session only to disconnect it.
                        let accept_slot = match connection_sender.try_reserve() {
                            Ok(slot) => slot,
                            Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => continue,
                            Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => break,
                        };

                        let packet = crate::packet::OpenConnectionReply2 {
                            guid,
                            address: addr,
                            mtu: req.mtu,
                            encryption_enabled: 0x00,
                        };

                        let reply = match write_packet_connection_open_reply_2(&packet) {
                            Ok(p) => p,
                            Err(_) => continue,
                        };

                        if let Err(e) = socket.send_to(&reply, addr).await {
                            raknet_log_error!("udp socket send_to error : {}", e);
                            continue;
                        }

                        // Cover the 64-datagram flight window plus control traffic.
                        let (sender, receiver) = channel::<Vec<u8>>(128);
                        let raknet_version = pending_versions
                            .remove(&addr)
                            .filter(|(_, time)| time.elapsed().as_secs() < 60)
                            .map(|(version, _)| version)
                            .unwrap_or(RAKNET_PROTOCOL_VERSION);
                        version_map.lock().await.insert(addr, raknet_version);

                        let raknet_socket = RaknetSocket::from(
                            &addr,
                            &socket,
                            receiver,
                            req.mtu,
                            collect_sender.clone(),
                            raknet_version,
                        )
                        .await;

                        sessions.lock().await.insert(
                            addr,
                            SessionSender {
                                sender,
                                close: raknet_socket.close_signal(),
                                guid: req.guid,
                                mtu: req.mtu,
                            },
                        );

                        raknet_log_debug!("accept connection : {}", addr);
                        accept_slot.send(raknet_socket);
                    }
                    PacketID::Disconnect => {
                        dispatch_cache.forget(&addr);
                        let session_sender = sessions
                            .lock()
                            .await
                            .remove(&addr)
                            .map(|session| session.close);
                        if let Some(session_sender) = session_sender {
                            session_sender.close();
                            version_map.lock().await.remove(&addr);
                        }
                    }
                    _ => {
                        let result = if let Some(sender) = dispatch_cache.sender(&addr) {
                            // Preserve the cooperative budget consumed by the map
                            // lock on the uncached path. A busy UDP listener must
                            // still give protocol and application tasks time to run.
                            tokio::task::consume_budget().await;
                            Some(sender.try_send(buf[..size].to_vec()))
                        } else {
                            dispatch_cache.forget(&addr);
                            let sessions = sessions.lock().await;
                            sessions.get(&addr).map(|session| {
                                let result = session.sender.try_send(buf[..size].to_vec());
                                dispatch_cache.remember(addr, session);
                                result
                            })
                        };
                        match result {
                            Some(Ok(())) | None => {}
                            Some(Err(tokio::sync::mpsc::error::TrySendError::Full(_))) => {
                                raknet_log_debug!("session receive queue full for {}", addr);
                            }
                            Some(Err(tokio::sync::mpsc::error::TrySendError::Closed(_))) => {
                                dispatch_cache.forget(&addr);
                                sessions.lock().await.remove(&addr);
                                version_map.lock().await.remove(&addr);
                            }
                        }
                    }
                }
            }
            raknet_log_debug!("listen worker closed");
        });
    }

    /// Wait for and accept the next incoming RakNet connection.
    ///
    /// Call [`listen`](Self::listen) first.
    ///
    /// # Example
    /// ```ignore
    /// let mut listener = RaknetListener::bind("127.0.0.1:19132".parse().unwrap()).await.unwrap();
    /// listener.listen().await;
    /// let mut socket = listener.accept().await.unwrap();
    /// ```
    pub async fn accept(&mut self) -> Result<RaknetSocket> {
        if !self.listened {
            Err(RaknetError::NotListen)
        } else {
            tokio::select! {
                a = self.connection_receiver.recv() => {
                    match a {
                        Some(p) => Ok(p),
                        None => {
                            Err(RaknetError::NotListen)
                        },
                    }
                },
                _ = self.close_notifier.acquire() => {
                    raknet_log_debug!("accept close notified");
                    Err(RaknetError::NotListen)
                }
            }
        }
    }

    /// Get the GUID, a random number that uniquely identifies the listener instance.
    pub fn get_guid(&self) -> u64 {
        self.guid
    }

    /// Set the server-list MOTD returned in unconnected pong packets.
    ///
    /// Future ping responses use the updated value.
    ///
    /// # Example
    /// ```ignore
    /// let mut listener = RaknetListener::bind("127.0.0.1:19132".parse().unwrap()).await.unwrap();
    /// listener.set_motd("Bedrock Server", 20, "protocol", "version", "Survival", 19132).await?;
    /// ```
    pub async fn set_motd(
        &mut self,
        server_name: &str,
        max_connection: u32,
        mc_protocol_version: &str,
        mc_version: &str,
        game_type: &str,
        port: u16,
    ) -> Result<()> {
        let motd = format!(
            "MCPE;{};{};{};0;{};{};Bedrock level;{};1;{};",
            server_name,
            mc_protocol_version,
            mc_version,
            max_connection,
            self.guid,
            game_type,
            port
        );

        self.motd = motd.clone();
        self.motd_sender.send_replace(motd);
        Ok(())
    }

    /// Return the MOTD sent in unconnected pong packets.
    ///
    /// # Example
    /// ```ignore
    /// let listener = RaknetListener::bind("127.0.0.1:19132".parse().unwrap()).await.unwrap();
    /// let motd = listener.get_motd().await;
    /// ```
    pub async fn get_motd(&self) -> String {
        self.motd.clone()
    }

    /// Return the local UDP address used by this listener.
    ///
    /// # Example
    /// ```ignore
    /// let mut socket = RaknetListener::bind("127.0.0.1:19132".parse().unwrap()).await.unwrap();
    /// assert_eq!(socket.local_addr().unwrap().ip(), IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)));
    /// ```
    pub fn local_addr(&self) -> Result<SocketAddr> {
        self.socket
            .as_ref()
            .ok_or(RaknetError::ConnectionClosed)?
            .local_addr()
            .map_err(|_| RaknetError::SocketError)
    }

    /// Close the listener and all active connections.
    ///
    /// # Example
    /// ```ignore
    /// let mut socket = RaknetListener::bind("127.0.0.1:19132".parse().unwrap()).await.unwrap();
    /// socket.close().await;
    /// ```
    pub async fn close(&mut self) -> Result<()> {
        // Signal the whole group before releasing any socket. Active flows must
        // never be remapped onto a shard that is still accepting new sessions.
        self.close_notifier.close();
        for shard in &self.shards {
            shard.close_notifier.close();
        }
        self.close_single().await?;
        for shard in &mut self.shards {
            shard.close_single().await?;
        }
        Ok(())
    }

    async fn close_single(&mut self) -> Result<()> {
        if self.socket.is_none() {
            return Ok(());
        }
        self.close_notifier.close();
        if self.listened {
            self.all_session_closed_notifier.notified().await;
        }
        if let Some(socket) = self.socket.as_ref() {
            while Arc::strong_count(socket) != 1 {
                tokio::task::yield_now().await;
            }
        }
        self.socket = None;
        self.listened = false;
        Ok(())
    }

    /// Set the full MOTD string used by future unconnected pong responses.
    ///
    /// # Example
    /// ```ignore
    /// let mut listener = RaknetListener::bind("127.0.0.1:19132".parse().unwrap()).await.unwrap();
    /// listener.set_full_motd(String::from("motd")).await.unwrap();
    /// ```
    pub async fn set_full_motd(&mut self, motd: String) -> Result<()> {
        self.motd = motd.clone();
        self.motd_sender.send_replace(motd);
        Ok(())
    }

    pub async fn get_peer_raknet_version(&self, peer: &SocketAddr) -> Result<u8> {
        if let Some(version) = self.version_map.lock().await.get(peer).copied() {
            return Ok(version);
        }
        for shard in &self.shards {
            if let Some(version) = shard.version_map.lock().await.get(peer).copied() {
                return Ok(version);
            }
        }
        Ok(RAKNET_PROTOCOL_VERSION)
    }
}

impl Drop for RaknetListener {
    fn drop(&mut self) {
        self.close_notifier.close();
    }
}

#[tokio::test]
async fn supplied_udp_socket_keeps_its_receive_buffer_configuration() {
    let socket = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    socket2::SockRef::from(&socket)
        .set_recv_buffer_size(8192)
        .unwrap();
    let before = socket2::SockRef::from(&socket).recv_buffer_size().unwrap();
    let listener = RaknetListener::from_std(socket).await.unwrap();
    let socket = listener.socket.as_ref().unwrap();
    assert_eq!(
        socket2::SockRef::from(socket.as_ref())
            .recv_buffer_size()
            .unwrap(),
        before
    );
}

#[tokio::test]
async fn custom_receive_buffer_binds_and_rejects_unrepresentable_sizes() {
    let address = "127.0.0.1:0".parse().unwrap();
    let listener = RaknetListener::bind_with_receive_buffer_size(
        &address,
        std::num::NonZeroUsize::new(8192).unwrap(),
    )
    .await
    .unwrap();
    assert_ne!(listener.local_addr().unwrap().port(), 0);
    assert!(matches!(
        RaknetListener::bind_with_receive_buffer_size(
            &address,
            std::num::NonZeroUsize::new(usize::MAX).unwrap(),
        )
        .await,
        Err(RaknetError::SocketError)
    ));
}

#[cfg(all(test, target_os = "linux"))]
mod shard_tests {
    use super::*;
    use crate::arq::Reliability;
    use std::{num::NonZeroUsize, time::Duration};
    use tokio::{task::JoinSet, time::timeout};

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn shards_share_discovery_backlog_versions_and_close_all_sessions() {
        timeout(Duration::from_secs(10), async {
            let mut listener = RaknetListener::bind_with_socket_shards(
                &"127.0.0.1:0".parse().unwrap(),
                NonZeroUsize::new(4).unwrap(),
            )
            .await
            .unwrap()
            .with_accept_backlog(NonZeroUsize::new(64).unwrap());
            let address = listener.local_addr().unwrap();
            let guid = listener.get_guid();
            listener
                .set_full_motd("shared discovery".into())
                .await
                .unwrap();
            listener.listen().await;
            listener
                .set_full_motd("updated discovery".into())
                .await
                .unwrap();
            for _ in 0..32 {
                let socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
                let packet =
                    write_packet_ping(&PacketUnconnectedPing { time: 1, guid: 2 }).unwrap();
                socket.send_to(&packet, address).await.unwrap();
                let mut buffer = [0; 2048];
                let (size, _) = socket.recv_from(&mut buffer).await.unwrap();
                let pong = read_packet_pong(&buffer[..size]).unwrap();
                assert_eq!(pong.guid, guid);
                assert_eq!(pong.motd, "updated discovery");
            }
            let mut tasks = JoinSet::new();
            for index in 0..32 {
                tasks.spawn(async move {
                    let version = if index % 2 == 0 { 10 } else { 11 };
                    RaknetSocket::connect_with_version(&address, version)
                        .await
                        .unwrap()
                });
            }
            let mut servers = Vec::new();
            for _ in 0..32 {
                let server = listener.accept().await.unwrap();
                assert_eq!(
                    listener
                        .get_peer_raknet_version(&server.peer_addr().unwrap())
                        .await
                        .unwrap(),
                    server.raknet_version().unwrap()
                );
                servers.push(server);
            }
            let mut clients = Vec::new();
            while let Some(client) = tasks.join_next().await {
                clients.push(client.unwrap());
            }
            let mut occupied = usize::from(!listener.sessions.lock().await.is_empty());
            for shard in &listener.shards {
                occupied += usize::from(!shard.sessions.lock().await.is_empty());
            }
            assert!(occupied > 1, "flows did not reach multiple sockets");
            for server in &servers {
                server
                    .send(&[0xfe, 42], Reliability::ReliableOrdered)
                    .await
                    .unwrap();
            }
            for client in &clients {
                assert_eq!(client.recv().await.unwrap(), vec![0xfe, 42]);
            }
            listener.close().await.unwrap();
            listener.close().await.unwrap();
            assert!(listener.sessions.lock().await.is_empty());
            assert!(listener.socket.is_none());
            for shard in &listener.shards {
                assert!(shard.sessions.lock().await.is_empty());
                assert!(shard.socket.is_none());
            }
            for client in clients {
                assert!(client.recv().await.is_err());
            }
        })
        .await
        .expect("sharded listener stalled");
    }

    #[tokio::test]
    async fn shard_count_is_bounded_and_one_uses_the_normal_listener() {
        let address = "127.0.0.1:0".parse().unwrap();
        assert!(
            RaknetListener::bind_with_socket_shards(&address, NonZeroUsize::new(65).unwrap(),)
                .await
                .is_err()
        );
        let mut listener = RaknetListener::bind_with_socket_shards(&address, NonZeroUsize::MIN)
            .await
            .unwrap();
        assert!(listener.shards.is_empty());
        listener.close().await.unwrap();
    }
}

#[cfg(test)]
mod dispatch_cache_tests {
    use super::*;

    fn session() -> (SessionSender, Receiver<Vec<u8>>) {
        let (sender, receiver) = channel(2);
        (
            SessionSender {
                sender,
                close: Arc::new(tokio::sync::Semaphore::new(0)),
                guid: 1,
                mtu: 1400,
            },
            receiver,
        )
    }

    #[test]
    fn cache_rejects_closed_sessions_and_replaces_reused_addresses() {
        let address = "127.0.0.1:19132".parse().unwrap();
        let (first, _first_receiver) = session();
        let (second, second_receiver) = session();
        let mut cache = SessionDispatchCache::default();
        cache.remember(address, &first);
        assert!(cache.sender(&address).unwrap().same_channel(&first.sender));
        first.close.close();
        assert!(cache.sender(&address).is_none());
        cache.remember(address, &second);
        assert!(cache.sender(&address).unwrap().same_channel(&second.sender));
        drop(second_receiver);
        assert!(cache.sender(&address).is_none());
        cache.prune();
        assert!(cache.entries.iter().all(Option::is_none));
    }

    #[tokio::test]
    async fn late_collection_does_not_remove_a_replacement_peer() {
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            let mut listener = RaknetListener::bind(&"127.0.0.1:0".parse().unwrap())
                .await
                .unwrap();
            let peer_socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
            let marker_socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
            let peer = peer_socket.local_addr().unwrap();
            let marker = marker_socket.local_addr().unwrap();
            let (replacement, _replacement_receiver) = session();
            let close = replacement.close.clone();
            let (old_marker, _marker_receiver) = session();
            old_marker.close.close();
            listener.sessions.lock().await.insert(peer, replacement);
            listener.sessions.lock().await.insert(marker, old_marker);
            listener.version_map.lock().await.insert(peer, 11);
            let (collect_sender, collect_receiver) = channel(2);
            listener
                .start_session_collect(
                    listener.socket.as_ref().unwrap(),
                    &listener.sessions,
                    collect_receiver,
                )
                .await;
            collect_sender.send(peer).await.unwrap();
            // The marker makes it observable that the earlier notification was processed.
            collect_sender.send(marker).await.unwrap();
            while listener.sessions.lock().await.contains_key(&marker) {
                tokio::task::yield_now().await;
            }
            assert!(listener.sessions.lock().await.contains_key(&peer));
            assert_eq!(listener.version_map.lock().await.get(&peer), Some(&11));
            let mut packet = [0; 32];
            assert_eq!(
                peer_socket.try_recv_from(&mut packet).unwrap_err().kind(),
                std::io::ErrorKind::WouldBlock
            );
            close.close();
            collect_sender.send(peer).await.unwrap();
            while listener.sessions.lock().await.contains_key(&peer) {
                tokio::task::yield_now().await;
            }
            let (length, _) = peer_socket.recv_from(&mut packet).await.unwrap();
            assert_eq!(&packet[..length], &[PacketID::Disconnect.to_u8()]);
            drop(collect_sender);
            listener.close().await.unwrap();
        })
        .await
        .expect("session collection stalled");
    }

    #[test]
    fn collisions_never_dispatch_to_or_forget_another_peer() {
        let (first, _first_receiver) = session();
        let (second, _second_receiver) = session();
        let first_address = SocketAddr::from(([127, 0, 0, 1], 19132));
        let second_address = SocketAddr::from(([127, 0, 0, 2], 19132));
        let third_address = "[::1]:19132".parse().unwrap();
        let mut cache = SessionDispatchCache::default();
        cache.remember(first_address, &first);
        cache.remember(second_address, &second);
        assert!(cache.sender(&first_address).is_none());
        assert!(cache.sender(&third_address).is_none());
        assert!(
            cache
                .sender(&second_address)
                .unwrap()
                .same_channel(&second.sender)
        );
        cache.forget(&first_address);
        assert!(cache.sender(&second_address).is_some());
        cache.remember(third_address, &first);
        assert!(cache.sender(&second_address).is_none());
        assert!(
            cache
                .sender(&third_address)
                .unwrap()
                .same_channel(&first.sender)
        );
        for port in 1..=u16::MAX {
            cache.remember(SocketAddr::from(([127, 0, 0, 1], port)), &first);
        }
        assert_eq!(cache.entries.len(), SessionDispatchCache::LIMIT);
        assert_eq!(
            cache.entries.iter().filter(|entry| entry.is_some()).count(),
            SessionDispatchCache::LIMIT
        );
    }
}
