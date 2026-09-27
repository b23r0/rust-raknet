//! TCP signaling forwarding for Bedrock servers using NetherNet.
//!
//! NetherNet uses HTTP signaling to exchange WebRTC session descriptions, then
//! sends gameplay data directly between the client and the Bedrock host. This
//! proxy forwards only the TCP signaling connection to a NetherNet-capable
//! backend; it does not implement WebRTC or relay gameplay traffic.

use std::{io, net::SocketAddr};
use tokio::{
    io::copy_bidirectional,
    net::{TcpListener, TcpStream},
};

/// Forwards NetherNet's TCP signaling connections to a NetherNet-capable server.
///
/// The client must still be able to reach the backend's advertised WebRTC ICE
/// candidate directly for gameplay. This is useful when the signaling endpoint
/// needs a different local address or port from the backend.
///
/// # Example
/// ```no_run
/// # async fn example() -> std::io::Result<()> {
/// let proxy = rust_raknet::NetherNetProxy::bind(
///     "127.0.0.1:19144".parse().unwrap(),
///     "127.0.0.1:19142".parse().unwrap(),
/// ).await?;
/// proxy.run().await
/// # }
/// ```
pub struct NetherNetProxy {
    listener: TcpListener,
    upstream: SocketAddr,
}

impl NetherNetProxy {
    /// Binds a TCP signaling endpoint and forwards each connection to `upstream`.
    pub async fn bind(listen: SocketAddr, upstream: SocketAddr) -> io::Result<Self> {
        Ok(Self {
            listener: TcpListener::bind(listen).await?,
            upstream,
        })
    }

    /// Returns the local TCP address used for NetherNet signaling.
    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.listener.local_addr()
    }

    /// Accepts and forwards signaling connections until an I/O error occurs.
    ///
    /// Call this in a Tokio task if the application needs to stop it by
    /// cancelling that task.
    pub async fn run(&self) -> io::Result<()> {
        loop {
            let (client, peer) = self.listener.accept().await?;
            let upstream = self.upstream;
            tokio::spawn(async move {
                if let Err(error) = forward(client, upstream).await {
                    crate::raknet_log_error!(
                        "NetherNet signaling relay for {peer} failed: {error}"
                    );
                }
            });
        }
    }
}

async fn forward(mut client: TcpStream, upstream: SocketAddr) -> io::Result<()> {
    let mut upstream_stream = TcpStream::connect(upstream).await?;
    copy_bidirectional(&mut client, &mut upstream_stream).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::NetherNetProxy;
    use std::time::Duration;
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::{TcpListener, TcpStream},
        time::timeout,
    };

    #[tokio::test]
    async fn closes_client_when_upstream_is_unavailable() {
        let unavailable = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let upstream_addr = unavailable.local_addr().unwrap();
        drop(unavailable);

        let proxy = NetherNetProxy::bind("127.0.0.1:0".parse().unwrap(), upstream_addr)
            .await
            .unwrap();
        let proxy_addr = proxy.local_addr().unwrap();
        let proxy_task = tokio::spawn(async move { proxy.run().await });

        let result = timeout(Duration::from_secs(3), async {
            let mut client = TcpStream::connect(proxy_addr).await.unwrap();
            client
                .write_all(b"GET /v1/join HTTP/1.1\r\n\r\n")
                .await
                .unwrap();
            let mut response = Vec::new();
            client.read_to_end(&mut response).await.map(|_| response)
        })
        .await
        .expect("client was not closed after the upstream connection failed");

        assert!(result.is_err() || result.as_ref().is_ok_and(Vec::is_empty));
        proxy_task.abort();
        let _ = proxy_task.await;
    }

    #[tokio::test]
    async fn forwards_signaling_bytes_in_both_directions() {
        let upstream = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let upstream_addr = upstream.local_addr().unwrap();
        let upstream_task = tokio::spawn(async move {
            let (mut stream, _) = upstream.accept().await.unwrap();
            let mut request = Vec::new();
            let mut chunk = [0; 512];
            loop {
                let count = stream.read(&mut chunk).await.unwrap();
                if count == 0 {
                    break;
                }
                request.extend_from_slice(&chunk[..count]);
                if request.windows(4).any(|window| window == b"\r\n\r\n") {
                    break;
                }
            }
            assert!(request.starts_with(b"GET /v1/join HTTP/1.1\r\n"));
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok")
                .await
                .unwrap();
        });

        let proxy = NetherNetProxy::bind("127.0.0.1:0".parse().unwrap(), upstream_addr)
            .await
            .unwrap();
        let proxy_addr = proxy.local_addr().unwrap();
        let proxy_task = tokio::spawn(async move { proxy.run().await });

        let result = timeout(Duration::from_secs(3), async {
            let mut client = TcpStream::connect(proxy_addr).await.unwrap();
            client
                .write_all(b"GET /v1/join HTTP/1.1\r\nHost: localhost\r\n\r\n")
                .await
                .unwrap();
            client.shutdown().await.unwrap();
            let mut response = Vec::new();
            client.read_to_end(&mut response).await.unwrap();
            response
        })
        .await
        .expect("signaling forward timed out");

        assert!(result.starts_with(b"HTTP/1.1 200 OK\r\n"));
        assert!(result.ends_with(b"\r\nok"));
        upstream_task.await.unwrap();
        proxy_task.abort();
        let _ = proxy_task.await;
    }
}
