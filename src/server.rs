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

type SessionSender = (i64, Sender<Vec<u8>>);

/// A RakNet UDP server that accepts incoming connections.
pub struct RaknetListener {
    motd: String,
    socket: Option<Arc<UdpSocket>>,
    guid: u64,
    listened: bool,
    connection_receiver: Receiver<RaknetSocket>,
    connection_sender: Sender<RaknetSocket>,
    sessions: Arc<Mutex<HashMap<SocketAddr, SessionSender>>>,
    close_notifier: Arc<tokio::sync::Semaphore>,
    all_session_closed_notifier: Arc<Notify>,
    drop_notifier: Arc<Notify>,
    version_map: Arc<Mutex<HashMap<String, u8>>>,
    motd_receiver: watch::Receiver<String>,
    motd_sender: watch::Sender<String>,
}

impl RaknetListener {
    /// Bind a RakNet listener to the specified UDP address.
    ///
    /// Call [`listen`](Self::listen) before accepting connections.
    pub async fn bind(sockaddr: &SocketAddr) -> Result<Self> {
        let socket = UdpSocket::bind(sockaddr)
            .await
            .map_err(|_| RaknetError::BindAddressError)?;
        Self::from_udp_socket(socket).await
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
        let (connection_sender, connection_receiver) = channel::<RaknetSocket>(10);
        let (motd_sender, motd_receiver) = watch::channel(String::new());
        let listener = Self {
            motd: String::new(),
            socket: Some(Arc::new(socket)),
            guid: rand::random(),
            listened: false,
            connection_receiver,
            connection_sender,
            sessions: Arc::new(Mutex::new(HashMap::new())),
            close_notifier: Arc::new(tokio::sync::Semaphore::new(0)),
            all_session_closed_notifier: Arc::new(Notify::new()),
            drop_notifier: Arc::new(Notify::new()),
            version_map: Arc::new(Mutex::new(HashMap::new())),
            motd_receiver,
            motd_sender,
        };

        listener.drop_watcher().await;
        Ok(listener)
    }

    async fn start_session_collect(
        &self,
        socket: &Arc<UdpSocket>,
        sessions: &Arc<Mutex<HashMap<SocketAddr, SessionSender>>>,
        mut collect_receiver: Receiver<SocketAddr>,
    ) {
        let sessions = sessions.clone();
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
                if sessions.contains_key(&addr) {
                    match socket.send_to(&[PacketID::Disconnect.to_u8()], addr).await {
                        Ok(_) => {}
                        Err(e) => {
                            raknet_log_error!("udp socket send_to error : {}", e);
                        }
                    };
                    sessions.remove(&addr);
                    raknet_log_debug!("collect socket : {}", addr);
                }
            }

            let mut sessions = sessions.lock().await;

            for (addr, (_, sender)) in sessions.iter() {
                let _ = sender.send(vec![PacketID::Disconnect.to_u8()]).await;

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
                    raknet_log_debug!("collect socket : {}", addr);
                }
            }

            sessions.clear();
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

                let new_motd = motd_receiver.borrow_and_update().clone();
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
                            motd: new_motd,
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
                            motd: new_motd,
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
                        {
                            let mut version_map = version_map.lock().await;
                            version_map.insert(addr.to_string(), req.protocol_version);
                        }

                        let packet = crate::packet::OpenConnectionReply1 {
                            guid,
                            // Encryption is not negotiated by this implementation.
                            use_encryption: 0x00,
                            mtu_size: RAKNET_CLIENT_MTU,
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
                        let req = match read_packet_connection_open_request_2(&buf[..size]) {
                            Ok(p) => p,
                            Err(_) => continue,
                        };

                        let already_connected = { sessions.lock().await.contains_key(&addr) };
                        if already_connected {
                            if let Ok(packet) =
                                write_packet_already_connected(&AlreadyConnected { guid })
                            {
                                let _ = socket.send_to(&packet, addr).await;
                            }
                            continue;
                        }

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

                        let (sender, receiver) = channel::<Vec<u8>>(10);
                        let raknet_version = {
                            let version_map = version_map.lock().await;
                            *version_map
                                .get(&addr.to_string())
                                .unwrap_or(&RAKNET_PROTOCOL_VERSION)
                        };

                        let raknet_socket = RaknetSocket::from(
                            &addr,
                            &socket,
                            receiver,
                            req.mtu,
                            collect_sender.clone(),
                            raknet_version,
                        )
                        .await;

                        sessions
                            .lock()
                            .await
                            .insert(addr, (cur_timestamp_millis(), sender));

                        raknet_log_debug!("accept connection : {}", addr);
                        if connection_sender.try_send(raknet_socket).is_err() {
                            sessions.lock().await.remove(&addr);
                            let _ = socket.send_to(&[PacketID::Disconnect.to_u8()], addr).await;
                            raknet_log_debug!(
                                "pending accept queue is full; disconnected {}",
                                addr
                            );
                        }
                    }
                    PacketID::Disconnect => {
                        let session_sender = sessions
                            .lock()
                            .await
                            .remove(&addr)
                            .map(|(_, sender)| sender);
                        if let Some(session_sender) = session_sender {
                            let _ = session_sender.try_send(buf[..size].to_vec());
                        }
                    }
                    _ => {
                        let session_sender = {
                            let mut sessions = sessions.lock().await;
                            sessions.get_mut(&addr).map(|(last_seen, sender)| {
                                *last_seen = cur_timestamp_millis();
                                sender.clone()
                            })
                        };

                        if let Some(session_sender) = session_sender {
                            match session_sender.try_send(buf[..size].to_vec()) {
                                Ok(()) => {}
                                Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => {
                                    raknet_log_debug!("session receive queue full for {}", addr);
                                }
                                Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => {
                                    sessions.lock().await.remove(&addr);
                                }
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
        if self.close_notifier.is_closed() {
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
        let version_map = self.version_map.lock().await;
        let ver = version_map.get(&peer.to_string());
        Ok(*ver.unwrap_or(&RAKNET_PROTOCOL_VERSION))
    }

    async fn drop_watcher(&self) {
        let close_notifier = self.close_notifier.clone();
        let drop_notifier = self.drop_notifier.clone();
        tokio::spawn(async move {
            raknet_log_debug!("listener drop watcher start");
            drop_notifier.notify_one();

            drop_notifier.notified().await;

            if close_notifier.is_closed() {
                raknet_log_debug!("close notifier closed");
                return;
            }

            close_notifier.close();

            raknet_log_debug!("listener drop watcher closed");
        });

        self.drop_notifier.notified().await;
    }
}

impl Drop for RaknetListener {
    fn drop(&mut self) {
        self.drop_notifier.notify_one();
    }
}
