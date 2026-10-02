//! Bounded batches of independent UDP datagrams. No batching timer is added.

#[cfg(target_os = "linux")]
mod linux {
    use crate::arq::FrameSetPacket;
    use std::{io, net::SocketAddr, os::fd::AsRawFd};
    use tokio::{io::Interest, net::UdpSocket};

    const BATCH: usize = 16;
    const BUFFER_SIZE: usize = 2048;

    struct BatchReceiver {
        buffers: Vec<[u8; BUFFER_SIZE]>,
        lengths: [usize; BATCH],
        addresses: [SocketAddr; BATCH],
        next: usize,
        count: usize,
    }

    impl BatchReceiver {
        pub(crate) fn new() -> Self {
            Self {
                buffers: vec![[0; BUFFER_SIZE]; BATCH],
                lengths: [0; BATCH],
                addresses: [SocketAddr::from(([0, 0, 0, 0], 0)); BATCH],
                next: 0,
                count: 0,
            }
        }

        pub(crate) async fn recv_from(
            &mut self,
            socket: &UdpSocket,
            output: &mut [u8],
        ) -> io::Result<(usize, SocketAddr)> {
            if self.next == self.count {
                self.count = socket
                    .async_io(Interest::READABLE, || {
                        loop {
                            match self.receive_ready(socket) {
                                Err(error) if error.kind() == io::ErrorKind::Interrupted => {
                                    continue;
                                }
                                result => break result,
                            }
                        }
                    })
                    .await?;
                self.next = 0;
            }
            let index = self.next;
            self.next += 1;
            let length = self.lengths[index].min(output.len());
            output[..length].copy_from_slice(&self.buffers[index][..length]);
            Ok((length, self.addresses[index]))
        }

        fn receive_ready(&mut self, socket: &UdpSocket) -> io::Result<usize> {
            // Zero is valid for these C ABI structs. All pointers used by the
            // syscall are set below and remain live until it returns.
            let mut addresses: [libc::sockaddr_storage; BATCH] = unsafe { std::mem::zeroed() };
            let mut vectors: [libc::iovec; BATCH] = unsafe { std::mem::zeroed() };
            let mut messages: [libc::mmsghdr; BATCH] = unsafe { std::mem::zeroed() };
            for index in 0..BATCH {
                vectors[index].iov_base = self.buffers[index].as_mut_ptr().cast();
                vectors[index].iov_len = BUFFER_SIZE;
                messages[index].msg_hdr.msg_name =
                    (&mut addresses[index] as *mut libc::sockaddr_storage).cast();
                messages[index].msg_hdr.msg_namelen =
                    std::mem::size_of::<libc::sockaddr_storage>() as libc::socklen_t;
                messages[index].msg_hdr.msg_iov = &mut vectors[index];
                messages[index].msg_hdr.msg_iovlen = 1;
            }
            // The fd is borrowed from Tokio. MSG_DONTWAIT never blocks the
            // worker; async_io manages readiness and retries WouldBlock.
            let count = unsafe {
                libc::recvmmsg(
                    socket.as_raw_fd(),
                    messages.as_mut_ptr(),
                    BATCH as u32,
                    libc::MSG_DONTWAIT,
                    std::ptr::null_mut(),
                )
            };
            if count < 0 {
                return Err(io::Error::last_os_error());
            }
            for index in 0..count as usize {
                let length = messages[index].msg_hdr.msg_namelen;
                let minimum = match addresses[index].ss_family as i32 {
                    libc::AF_INET => std::mem::size_of::<libc::sockaddr_in>(),
                    libc::AF_INET6 => std::mem::size_of::<libc::sockaddr_in6>(),
                    _ => {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            "unsupported UDP peer address",
                        ));
                    }
                };
                if (length as usize) < minimum
                    || length as usize > std::mem::size_of::<libc::sockaddr_storage>()
                {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "invalid UDP peer address",
                    ));
                }
                // The kernel initialized the address and its checked length.
                let mut storage = socket2::SockAddrStorage::zeroed();
                // Both wrappers have the platform sockaddr_storage layout.
                unsafe {
                    *storage.view_as::<libc::sockaddr_storage>() = addresses[index];
                }
                let address = unsafe { socket2::SockAddr::new(storage, length) };
                self.addresses[index] = address.as_socket().ok_or_else(|| {
                    io::Error::new(io::ErrorKind::InvalidData, "unsupported UDP peer address")
                })?;
                self.lengths[index] = (messages[index].msg_len as usize).min(BUFFER_SIZE);
            }
            Ok(count as usize)
        }
    }

    pub(crate) struct DatagramReceiver {
        batch: Option<Box<BatchReceiver>>,
    }

    impl DatagramReceiver {
        pub(crate) fn new(batching: bool) -> Self {
            Self {
                batch: batching.then(|| Box::new(BatchReceiver::new())),
            }
        }

        pub(crate) async fn recv_from(
            &mut self,
            socket: &UdpSocket,
            output: &mut [u8],
        ) -> io::Result<(usize, SocketAddr)> {
            match &mut self.batch {
                Some(batch) => batch.recv_from(socket, output).await,
                None => socket.recv_from(output).await,
            }
        }
    }

    pub(crate) async fn send_frames(
        socket: &UdpSocket,
        frames: &[FrameSetPacket],
        peer: &SocketAddr,
    ) -> io::Result<()> {
        let mut sent = 0;
        while sent < frames.len() {
            let batch = &frames[sent..frames.len().min(sent + BATCH)];
            let count = socket
                .async_io(Interest::WRITABLE, || {
                    loop {
                        match send_ready(socket, batch, peer) {
                            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                            result => break result,
                        }
                    }
                })
                .await?;
            if count == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::WriteZero,
                    "empty UDP send batch",
                ));
            }
            sent += count;
        }
        Ok(())
    }

    fn send_ready(
        socket: &UdpSocket,
        frames: &[FrameSetPacket],
        peer: &SocketAddr,
    ) -> io::Result<usize> {
        let mut headers = [[0u8; 32]; BATCH];
        // C structs contain only integers and pointers; null initialization is
        // valid. No raw pointer survives this synchronous syscall wrapper.
        let mut vectors: [[libc::iovec; 2]; BATCH] = unsafe { std::mem::zeroed() };
        let mut messages: [libc::mmsghdr; BATCH] = unsafe { std::mem::zeroed() };
        let address = socket2::SockAddr::from(*peer);
        for (index, frame) in frames.iter().enumerate() {
            let length = frame
                .encode_header(&mut headers[index])
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
            vectors[index][0] = libc::iovec {
                iov_base: headers[index].as_mut_ptr().cast(),
                iov_len: length,
            };
            vectors[index][1] = libc::iovec {
                iov_base: frame.data.as_ptr().cast_mut().cast(),
                iov_len: frame.data.len(),
            };
            messages[index].msg_hdr.msg_name = address.as_ptr().cast_mut().cast();
            messages[index].msg_hdr.msg_namelen = address.len();
            messages[index].msg_hdr.msg_iov = vectors[index].as_mut_ptr();
            messages[index].msg_hdr.msg_iovlen = 2;
        }
        // sendmmsg reads the headers/payloads and sockaddr; it does not modify
        // those bytes. Partial completion is returned and retried by the caller.
        let count = unsafe {
            libc::sendmmsg(
                socket.as_raw_fd(),
                messages.as_mut_ptr(),
                frames.len() as u32,
                libc::MSG_DONTWAIT,
            )
        };
        if count < 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(count as usize)
        }
    }
}

#[cfg(target_os = "linux")]
pub(crate) use linux::{DatagramReceiver, send_frames};

#[cfg(not(target_os = "linux"))]
pub(crate) struct DatagramReceiver;

#[cfg(not(target_os = "linux"))]
impl DatagramReceiver {
    pub(crate) fn new(_batching: bool) -> Self {
        Self
    }

    pub(crate) async fn recv_from(
        &mut self,
        socket: &tokio::net::UdpSocket,
        output: &mut [u8],
    ) -> std::io::Result<(usize, std::net::SocketAddr)> {
        socket.recv_from(output).await
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    use crate::arq::{FrameSetPacket, Reliability};
    use tokio::net::UdpSocket;

    #[tokio::test]
    async fn batches_preserve_every_datagram_and_peer_on_both_address_families() {
        for host in ["127.0.0.1:0", "[::1]:0"] {
            let sender = UdpSocket::bind(host).await.unwrap();
            let receiver = UdpSocket::bind(host).await.unwrap();
            let peer = receiver.local_addr().unwrap();
            let frames: Vec<_> = (0..40)
                .map(|index| {
                    let mut frame =
                        FrameSetPacket::new(Reliability::ReliableOrdered, vec![0xfe, index]);
                    frame.sequence_number = index as u32;
                    frame
                })
                .collect();
            send_frames(&sender, &frames, &peer).await.unwrap();
            let mut input = DatagramReceiver::new(true);
            let mut bytes = [0; 2048];
            for frame in frames {
                let (length, address) = input.recv_from(&receiver, &mut bytes).await.unwrap();
                assert_eq!(address, sender.local_addr().unwrap());
                assert_eq!(&bytes[..length], frame.serialize().unwrap());
            }
            assert!(receiver.try_recv_from(&mut bytes).is_err());
        }
    }
}
