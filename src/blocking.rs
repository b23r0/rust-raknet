//! Synchronous adapters backed by a shared, continuously running Tokio runtime.
//!
//! Enable the `blocking` feature. Construct one [`RaknetRuntime`] and reuse it
//! for listeners and clients. All blocking operations reject calls made inside
//! a Tokio runtime; use the asynchronous API there instead.
//!
//! Sending accepts data into the existing transport path. Only `flush` waits
//! for reliable acknowledgements. A timeout cancels the wait, not data already
//! queued: retrying a timed-out send can duplicate application messages, and a
//! timed-out batch may have sent a prefix. Concurrent sends retain the async
//! API's ordering semantics; serialize dependent messages on the same channel.
//!
//! ```no_run
//! use rust_raknet::{blocking::RaknetRuntime, Reliability};
//! # fn main() -> rust_raknet::blocking::Result<()> {
//! let runtime = RaknetRuntime::new()?;
//! let socket = runtime.connect(&"127.0.0.1:19132".parse().unwrap())?;
//! socket.send(&[0xfe, 1], Reliability::ReliableOrdered)?;
//! socket.flush()?;
//! socket.close()?;
//! # Ok(()) }
//! ```

use crate::{Bytes, Reliability, error::RaknetError};
use std::{future::Future, net::SocketAddr, num::NonZeroUsize, sync::Arc, time::Duration};

/// Errors from the synchronous adapter or the underlying protocol.
#[derive(Debug)]
pub enum Error {
    /// A protocol or socket operation failed.
    Raknet(RaknetError),
    /// Creating the runtime failed.
    Runtime(std::io::Error),
    /// Blocking inside a Tokio runtime is unsupported.
    AsyncContext,
    /// The operation did not complete before its deadline.
    Timeout,
}
/// Result of a synchronous transport operation.
pub type Result<T> = std::result::Result<T, Error>;
impl From<RaknetError> for Error {
    fn from(error: RaknetError) -> Self {
        Self::Raknet(error)
    }
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Raknet(e) => e.fmt(f),
            Self::Runtime(e) => write!(f, "failed to create Tokio runtime: {e}"),
            Self::AsyncContext => {
                f.write_str("blocking RakNet operations cannot run inside a Tokio runtime")
            }
            Self::Timeout => f.write_str("blocking RakNet operation timed out"),
        }
    }
}
impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Raknet(e) => Some(e),
            Self::Runtime(e) => Some(e),
            _ => None,
        }
    }
}
struct RuntimeOwner(Option<tokio::runtime::Runtime>);
impl Drop for RuntimeOwner {
    fn drop(&mut self) {
        // Dropping adapters in an async context must not panic or block a worker.
        // Explicit close/flush is required for graceful protocol shutdown.
        if let Some(runtime) = self.0.take() {
            runtime.shutdown_background();
        }
    }
}

/// Shared runtime owner. Clones share workers; sockets keep it alive themselves.
/// No global runtime is created. Dropping the last owner stops background work.
#[derive(Clone)]
pub struct RaknetRuntime {
    inner: Arc<RuntimeOwner>,
}
impl RaknetRuntime {
    /// Create a continuously running runtime with one network worker.
    pub fn new() -> Result<Self> {
        Self::with_worker_threads(NonZeroUsize::new(1).unwrap())
    }
    /// Create a shared runtime with an explicit worker count.
    pub fn with_worker_threads(workers: NonZeroUsize) -> Result<Self> {
        Self::check_context()?;
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(workers.get())
            .enable_all()
            .build()
            .map_err(Error::Runtime)?;
        Ok(Self {
            inner: Arc::new(RuntimeOwner(Some(runtime))),
        })
    }
    fn check_context() -> Result<()> {
        if tokio::runtime::Handle::try_current().is_ok() {
            Err(Error::AsyncContext)
        } else {
            Ok(())
        }
    }
    fn run<T>(&self, future: impl Future<Output = crate::error::Result<T>>) -> Result<T> {
        Self::check_context()?;
        self.inner
            .0
            .as_ref()
            .unwrap()
            .block_on(future)
            .map_err(Error::Raknet)
    }
    fn run_timeout<T>(
        &self,
        duration: Duration,
        future: impl Future<Output = crate::error::Result<T>>,
    ) -> Result<T> {
        Self::check_context()?;
        // Create the timer while entered into the owned runtime.
        self.inner.0.as_ref().unwrap().block_on(async {
            tokio::time::timeout(duration, future)
                .await
                .map_err(|_| Error::Timeout)?
                .map_err(Error::Raknet)
        })
    }
    /// Connect using the default RakNet version.
    pub fn connect(&self, address: &SocketAddr) -> Result<RaknetSocket> {
        let inner = self.run(crate::RaknetSocket::connect(address))?;
        Ok(RaknetSocket {
            inner,
            runtime: self.clone(),
        })
    }
    /// Connect with an overall handshake deadline.
    pub fn connect_timeout(
        &self,
        address: &SocketAddr,
        duration: Duration,
    ) -> Result<RaknetSocket> {
        let inner = self.run_timeout(duration, crate::RaknetSocket::connect(address))?;
        Ok(RaknetSocket {
            inner,
            runtime: self.clone(),
        })
    }
    /// Connect with an explicit protocol version.
    pub fn connect_with_version(&self, address: &SocketAddr, version: u8) -> Result<RaknetSocket> {
        let inner = self.run(crate::RaknetSocket::connect_with_version(address, version))?;
        Ok(RaknetSocket {
            inner,
            runtime: self.clone(),
        })
    }
    /// Connect with an explicit protocol version and requested MTU.
    pub fn connect_with_version_and_mtu(
        &self,
        address: &SocketAddr,
        version: u8,
        mtu: u16,
    ) -> Result<RaknetSocket> {
        let inner = self.run(crate::RaknetSocket::connect_with_version_and_mtu(
            address, version, mtu,
        ))?;
        Ok(RaknetSocket {
            inner,
            runtime: self.clone(),
        })
    }
    /// Bind a listener. Configure it, then call `listen` before `accept`.
    pub fn bind(&self, address: &SocketAddr) -> Result<RaknetListener> {
        let inner = self.run(crate::RaknetListener::bind(address))?;
        Ok(RaknetListener {
            inner,
            runtime: self.clone(),
        })
    }
    /// Bind with a maximum negotiated MTU.
    pub fn bind_with_maximum_mtu(&self, address: &SocketAddr, mtu: u16) -> Result<RaknetListener> {
        let inner = self.run(crate::RaknetListener::bind_with_maximum_mtu(address, mtu))?;
        Ok(RaknetListener {
            inner,
            runtime: self.clone(),
        })
    }
    /// Bind with a requested per-socket receive buffer size.
    pub fn bind_with_receive_buffer_size(
        &self,
        address: &SocketAddr,
        size: NonZeroUsize,
    ) -> Result<RaknetListener> {
        let inner = self.run(crate::RaknetListener::bind_with_receive_buffer_size(
            address, size,
        ))?;
        Ok(RaknetListener {
            inner,
            runtime: self.clone(),
        })
    }
    /// Bind a fixed group of Linux SO_REUSEPORT sockets.
    #[cfg(target_os = "linux")]
    pub fn bind_with_socket_shards(
        &self,
        address: &SocketAddr,
        count: NonZeroUsize,
    ) -> Result<RaknetListener> {
        let inner = self.run(crate::RaknetListener::bind_with_socket_shards(
            address, count,
        ))?;
        Ok(RaknetListener {
            inner,
            runtime: self.clone(),
        })
    }
    /// Adopt a standard UDP socket, preserving its buffer settings.
    pub fn from_std(&self, socket: std::net::UdpSocket) -> Result<RaknetListener> {
        let inner = self.run(crate::RaknetListener::from_std(socket))?;
        Ok(RaknetListener {
            inner,
            runtime: self.clone(),
        })
    }
    /// Discover a server using an unconnected ping, with a deadline.
    pub fn ping_timeout(&self, address: &SocketAddr, duration: Duration) -> Result<(i64, String)> {
        self.run_timeout(duration, crate::RaknetSocket::ping(address))
    }
}

/// Synchronous connection. Receive operations share one serialized consumer.
/// Batch receives wait for the first message, then drain ready messages.
/// This is a blocking API, not a nonblocking game-loop adapter.
/// Drop signals closure; call `flush` before `close` for acknowledged delivery.
pub struct RaknetSocket {
    inner: crate::RaknetSocket,
    runtime: RaknetRuntime,
}
impl RaknetSocket {
    /// Synchronous adapter for [`crate::RaknetSocket::send`].
    pub fn send(&self, buf: &[u8], reliability: Reliability) -> Result<()> {
        self.runtime.run(self.inner.send(buf, reliability))
    }
    /// Wait at most `duration`. Already queued data is not withdrawn on timeout.
    pub fn send_timeout(
        &self,
        buf: &[u8],
        reliability: Reliability,
        duration: Duration,
    ) -> Result<()> {
        self.runtime
            .run_timeout(duration, self.inner.send(buf, reliability))
    }
    /// Synchronous adapter for [`crate::RaknetSocket::send_with_order_channel`].
    pub fn send_with_order_channel(
        &self,
        buf: &[u8],
        reliability: Reliability,
        channel: u8,
    ) -> Result<()> {
        self.runtime.run(
            self.inner
                .send_with_order_channel(buf, reliability, channel),
        )
    }
    /// Synchronous adapter for [`crate::RaknetSocket::send_bytes`].
    pub fn send_bytes(&self, data: Bytes, reliability: Reliability) -> Result<()> {
        self.runtime.run(self.inner.send_bytes(data, reliability))
    }
    /// Wait at most `duration`. Already queued data is not withdrawn on timeout.
    pub fn send_bytes_timeout(
        &self,
        data: Bytes,
        reliability: Reliability,
        duration: Duration,
    ) -> Result<()> {
        self.runtime
            .run_timeout(duration, self.inner.send_bytes(data, reliability))
    }
    /// Synchronous adapter for [`crate::RaknetSocket::send_bytes_with_order_channel`].
    pub fn send_bytes_with_order_channel(
        &self,
        data: Bytes,
        reliability: Reliability,
        channel: u8,
    ) -> Result<()> {
        self.runtime.run(
            self.inner
                .send_bytes_with_order_channel(data, reliability, channel),
        )
    }
    /// Synchronous adapter for [`crate::RaknetSocket::send_batch`].
    pub fn send_batch(&self, messages: &[&[u8]], reliability: Reliability) -> Result<()> {
        self.runtime
            .run(self.inner.send_batch(messages, reliability))
    }
    /// Wait at most `duration`. Already queued data is not withdrawn on timeout.
    pub fn send_batch_timeout(
        &self,
        messages: &[&[u8]],
        reliability: Reliability,
        duration: Duration,
    ) -> Result<()> {
        self.runtime
            .run_timeout(duration, self.inner.send_batch(messages, reliability))
    }
    /// Synchronous adapter for [`crate::RaknetSocket::send_batch_with_order_channel`].
    pub fn send_batch_with_order_channel(
        &self,
        messages: &[&[u8]],
        reliability: Reliability,
        channel: u8,
    ) -> Result<()> {
        self.runtime.run(
            self.inner
                .send_batch_with_order_channel(messages, reliability, channel),
        )
    }
    /// Synchronous adapter for [`crate::RaknetSocket::send_bytes_batch`].
    pub fn send_bytes_batch(&self, messages: &[Bytes], reliability: Reliability) -> Result<()> {
        self.runtime
            .run(self.inner.send_bytes_batch(messages, reliability))
    }
    /// Synchronous adapter for [`crate::RaknetSocket::send_bytes_batch_with_order_channel`].
    pub fn send_bytes_batch_with_order_channel(
        &self,
        messages: &[Bytes],
        reliability: Reliability,
        channel: u8,
    ) -> Result<()> {
        self.runtime
            .run(
                self.inner
                    .send_bytes_batch_with_order_channel(messages, reliability, channel),
            )
    }
    /// Synchronous adapter for [`crate::RaknetSocket::recv`].
    pub fn recv(&self) -> Result<Vec<u8>> {
        self.runtime.run(self.inner.recv())
    }
    /// Wait at most `duration`. Already queued data is not withdrawn on timeout.
    pub fn recv_timeout(&self, duration: Duration) -> Result<Vec<u8>> {
        self.runtime.run_timeout(duration, self.inner.recv())
    }
    /// Synchronous adapter for [`crate::RaknetSocket::recv_bytes`].
    pub fn recv_bytes(&self) -> Result<Bytes> {
        self.runtime.run(self.inner.recv_bytes())
    }
    /// Wait at most `duration`. Already queued data is not withdrawn on timeout.
    pub fn recv_bytes_timeout(&self, duration: Duration) -> Result<Bytes> {
        self.runtime.run_timeout(duration, self.inner.recv_bytes())
    }
    /// Synchronous adapter for [`crate::RaknetSocket::recv_batch`].
    pub fn recv_batch(&self, output: &mut Vec<Vec<u8>>, limit: usize) -> Result<usize> {
        self.runtime.run(self.inner.recv_batch(output, limit))
    }
    /// Wait at most `duration`. Already queued data is not withdrawn on timeout.
    pub fn recv_batch_timeout(
        &self,
        output: &mut Vec<Vec<u8>>,
        limit: usize,
        duration: Duration,
    ) -> Result<usize> {
        self.runtime
            .run_timeout(duration, self.inner.recv_batch(output, limit))
    }
    /// Synchronous adapter for [`crate::RaknetSocket::recv_bytes_batch`].
    pub fn recv_bytes_batch(&self, output: &mut Vec<Bytes>, limit: usize) -> Result<usize> {
        self.runtime.run(self.inner.recv_bytes_batch(output, limit))
    }
    /// Synchronous adapter for [`crate::RaknetSocket::flush`].
    pub fn flush(&self) -> Result<()> {
        self.runtime.run(self.inner.flush())
    }
    /// Wait at most `duration`. Already queued data is not withdrawn on timeout.
    pub fn flush_timeout(&self, duration: Duration) -> Result<()> {
        self.runtime.run_timeout(duration, self.inner.flush())
    }
    /// Synchronous adapter for [`crate::RaknetSocket::close`].
    pub fn close(&self) -> Result<()> {
        self.runtime.run(self.inner.close())
    }
    /// Return the underlying connection metadata.
    pub fn peer_addr(&self) -> Result<SocketAddr> {
        self.inner.peer_addr().map_err(Error::Raknet)
    }
    /// Return the underlying connection metadata.
    pub fn local_addr(&self) -> Result<SocketAddr> {
        self.inner.local_addr().map_err(Error::Raknet)
    }
    /// Return the underlying connection metadata.
    pub fn raknet_version(&self) -> Result<u8> {
        self.inner.raknet_version().map_err(Error::Raknet)
    }
    /// Return the negotiated MTU.
    pub fn mtu(&self) -> u16 {
        self.inner.mtu()
    }
    /// Configure idle maintenance without blocking.
    pub fn set_idle_maintenance(&self, enabled: bool) {
        self.inner.set_idle_maintenance(enabled);
    }
    /// Apply optional per-connection policy through the shared runtime.
    #[cfg(feature = "send-policy")]
    pub fn set_send_options(&self, options: crate::SendOptions) -> Result<()> {
        self.runtime.run(self.inner.set_send_options(options))
    }
    /// Read the current per-connection policy.
    #[cfg(feature = "send-policy")]
    pub fn send_options(&self) -> Result<Option<crate::SendOptions>> {
        self.runtime
            .run(async { Ok(self.inner.send_options().await) })
    }
    /// Apply optional per-connection policy through the shared runtime.
    #[cfg(feature = "recovery-policy")]
    pub fn set_recovery_options(&self, options: crate::RecoveryOptions) -> Result<()> {
        self.runtime.run(self.inner.set_recovery_options(options))
    }
    /// Read the current per-connection policy.
    #[cfg(feature = "recovery-policy")]
    pub fn recovery_options(&self) -> Result<crate::RecoveryOptions> {
        self.runtime
            .run(async { Ok(self.inner.recovery_options().await) })
    }
}

/// Synchronous listener. Accepted sockets retain the same runtime owner.
pub struct RaknetListener {
    inner: crate::RaknetListener,
    runtime: RaknetRuntime,
}
impl RaknetListener {
    /// Configure the backlog before listening.
    pub fn with_accept_backlog(mut self, backlog: NonZeroUsize) -> Self {
        self.inner = self.inner.with_accept_backlog(backlog);
        self
    }
    /// Configure idle maintenance before listening.
    pub fn with_idle_maintenance(mut self, enabled: bool) -> Self {
        self.inner = self.inner.with_idle_maintenance(enabled);
        self
    }
    /// Configure receive batching before listening.
    #[cfg(target_os = "linux")]
    pub fn with_receive_batching(mut self, enabled: bool) -> Self {
        self.inner = self.inner.with_receive_batching(enabled);
        self
    }
    /// Start accepting incoming handshakes.
    pub fn listen(&mut self) -> Result<()> {
        self.runtime.run(async {
            self.inner.listen().await;
            Ok(())
        })
    }
    /// Wait for the next connection.
    pub fn accept(&mut self) -> Result<RaknetSocket> {
        let inner = self.runtime.run(self.inner.accept())?;
        Ok(RaknetSocket {
            inner,
            runtime: self.runtime.clone(),
        })
    }
    /// Wait for a connection for at most `duration`.
    pub fn accept_timeout(&mut self, duration: Duration) -> Result<RaknetSocket> {
        let inner = self.runtime.run_timeout(duration, self.inner.accept())?;
        Ok(RaknetSocket {
            inner,
            runtime: self.runtime.clone(),
        })
    }
    /// Close the listener and its connections, waiting for background cleanup.
    pub fn close(&mut self) -> Result<()> {
        self.runtime.run(self.inner.close())
    }
    /// Bound listener address.
    pub fn local_addr(&self) -> Result<SocketAddr> {
        self.inner.local_addr().map_err(Error::Raknet)
    }
    /// Server GUID.
    pub fn get_guid(&self) -> u64 {
        self.inner.get_guid()
    }
    /// Set the complete discovery response.
    pub fn set_full_motd(&mut self, motd: String) -> Result<()> {
        self.runtime.run(self.inner.set_full_motd(motd))
    }
    /// Read the discovery response.
    pub fn get_motd(&self) -> Result<String> {
        self.runtime.run(async { Ok(self.inner.get_motd().await) })
    }
    /// Read a peer's negotiated protocol version.
    pub fn get_peer_raknet_version(&self, peer: &SocketAddr) -> Result<u8> {
        self.runtime.run(self.inner.get_peer_raknet_version(peer))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sockets_share_workers_and_receive_progress_needs_no_block_on_call() {
        let runtime = RaknetRuntime::new().unwrap();
        let mut listener = runtime.bind(&"127.0.0.1:0".parse().unwrap()).unwrap();
        listener.listen().unwrap();
        let client = runtime
            .connect_timeout(&listener.local_addr().unwrap(), Duration::from_secs(10))
            .unwrap();
        let server = Arc::new(listener.accept_timeout(Duration::from_secs(10)).unwrap());
        assert!(Arc::ptr_eq(&runtime.inner, &client.runtime.inner));
        assert!(Arc::ptr_eq(&runtime.inner, &server.runtime.inner));
        let (delivered, received) = std::sync::mpsc::channel();
        let receiving = server.clone();
        runtime.inner.0.as_ref().unwrap().spawn(async move {
            delivered.send(receiving.inner.recv().await).unwrap();
        });
        drop(runtime);
        client
            .send(&[0xfe, 7], Reliability::ReliableOrdered)
            .unwrap();
        // Waiting on a standard channel never enters Tokio. Delivery must
        // progress on the shared runtime's background worker alone.
        assert_eq!(
            received
                .recv_timeout(Duration::from_secs(5))
                .unwrap()
                .unwrap(),
            [0xfe, 7]
        );
        client.flush_timeout(Duration::from_secs(5)).unwrap();
        listener.close().unwrap();
    }

    #[test]
    fn flush_timeout_does_not_withdraw_data_and_loss_recovery_keeps_running() {
        let runtime = RaknetRuntime::new().unwrap();
        let mut listener = runtime.bind(&"127.0.0.1:0".parse().unwrap()).unwrap();
        listener.listen().unwrap();
        let client = runtime
            .connect_timeout(&listener.local_addr().unwrap(), Duration::from_secs(10))
            .unwrap();
        let mut server = listener.accept_timeout(Duration::from_secs(10)).unwrap();
        // Discard all server-to-client packets, including reliable ACKs.
        server.inner.set_loss_rate(10);
        let payload = vec![0xfe; 4096];
        client.send(&payload, Reliability::ReliableOrdered).unwrap();
        assert_eq!(
            server.recv_timeout(Duration::from_secs(10)).unwrap(),
            payload
        );
        assert!(matches!(
            client.flush_timeout(Duration::from_millis(30)),
            Err(Error::Timeout)
        ));
        server.inner.set_loss_rate(0);
        // The queued data survives cancellation and background retries recover
        // the missing ACK without another application send.
        client.flush_timeout(Duration::from_secs(10)).unwrap();
        assert!(
            matches!(
                server.recv_timeout(Duration::from_millis(50)),
                Err(Error::Timeout)
            ),
            "reliable retransmission was delivered to the application twice"
        );
        listener.close().unwrap();
    }
}
