use crate::error::*;
use crate::utils::Endian;
use bytes::{Buf, BufMut};
use std::{
    io::{Cursor, Read},
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
};

#[derive(Clone, Default)]
pub struct RaknetWriter {
    buf: Vec<u8>,
}

impl RaknetWriter {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            buf: Vec::with_capacity(capacity),
        }
    }

    pub fn write(&mut self, v: &[u8]) -> Result<()> {
        self.buf.put_slice(v);
        Ok(())
    }

    pub fn write_u8(&mut self, v: u8) -> Result<()> {
        self.buf.put_u8(v);
        Ok(())
    }

    pub fn write_i16(&mut self, v: i16, n: Endian) -> Result<()> {
        match n {
            Endian::Big => {
                self.buf.put_i16(v);
                Ok(())
            }
            Endian::Little => {
                self.buf.put_i16_le(v);
                Ok(())
            }
        }
    }

    pub fn write_u16(&mut self, v: u16, n: Endian) -> Result<()> {
        match n {
            Endian::Big => {
                self.buf.put_u16(v);
                Ok(())
            }
            Endian::Little => {
                self.buf.put_u16_le(v);
                Ok(())
            }
        }
    }

    pub fn write_u24(&mut self, v: u32, n: Endian) -> Result<()> {
        match n {
            Endian::Big => {
                let a = v.to_be_bytes();
                self.buf.put_u8(a[1]);
                self.buf.put_u8(a[2]);
                self.buf.put_u8(a[3]);
            }
            Endian::Little => {
                let a = v.to_le_bytes();
                self.buf.put_u8(a[0]);
                self.buf.put_u8(a[1]);
                self.buf.put_u8(a[2]);
            }
        }
        Ok(())
    }

    pub fn write_u32(&mut self, v: u32, n: Endian) -> Result<()> {
        match n {
            Endian::Big => {
                self.buf.put_u32(v);
                Ok(())
            }
            Endian::Little => {
                self.buf.put_u32_le(v);
                Ok(())
            }
        }
    }

    pub fn write_i32(&mut self, v: i32, n: Endian) -> Result<()> {
        match n {
            Endian::Big => {
                self.buf.put_i32(v);
                Ok(())
            }
            Endian::Little => {
                self.buf.put_i32_le(v);
                Ok(())
            }
        }
    }

    pub fn write_i64(&mut self, v: i64, n: Endian) -> Result<()> {
        match n {
            Endian::Big => {
                self.buf.put_i64(v);
                Ok(())
            }
            Endian::Little => {
                self.buf.put_i64_le(v);
                Ok(())
            }
        }
    }

    pub fn write_magic(&mut self) -> Result<usize> {
        let magic: [u8; 16] = [
            0x00, 0xff, 0xff, 0x00, 0xfe, 0xfe, 0xfe, 0xfe, 0xfd, 0xfd, 0xfd, 0xfd, 0x12, 0x34,
            0x56, 0x78,
        ];
        self.buf.put_slice(&magic);
        Ok(magic.len())
    }

    pub fn write_u64(&mut self, v: u64, n: Endian) -> Result<()> {
        match n {
            Endian::Big => {
                self.buf.put_u64(v);
                Ok(())
            }
            Endian::Little => {
                self.buf.put_u64_le(v);
                Ok(())
            }
        }
    }

    pub fn write_string(&mut self, body: &str) -> Result<()> {
        let raw = body.as_bytes();
        let length = u16::try_from(raw.len()).map_err(|_| RaknetError::PacketParseError)?;
        self.buf.put_u16(length);
        self.buf.put_slice(raw);
        Ok(())
    }

    pub fn write_address(&mut self, address: SocketAddr) -> Result<()> {
        match address {
            SocketAddr::V4(address) => {
                self.write_u8(4)?;
                for octet in address.ip().octets() {
                    self.write_u8(0xff - octet)?;
                }
                self.write_u16(address.port(), Endian::Big)
            }
            SocketAddr::V6(address) => {
                self.write_u8(6)?;
                self.write_i16(23, Endian::Little)?;
                self.write_u16(address.port(), Endian::Big)?;
                self.write_i32(0, Endian::Big)?;
                self.write(&address.ip().octets())?;
                self.write_i32(0, Endian::Big)
            }
        }
    }

    pub fn get_raw_payload(self) -> Vec<u8> {
        self.buf
    }

    pub fn _pos(&self) -> u64 {
        self.buf.len() as u64
    }
}

pub struct RaknetReader<'a> {
    buf: Cursor<&'a [u8]>,
}

impl<'a> RaknetReader<'a> {
    pub(crate) fn remaining(&self) -> usize {
        self.buf.remaining()
    }

    pub fn new(buf: &'a [u8]) -> Self {
        Self {
            buf: Cursor::new(buf),
        }
    }
    pub fn read(&mut self, buf: &mut [u8]) -> Result<()> {
        match self.buf.read_exact(buf) {
            Ok(p) => Ok(p),
            Err(_) => Err(RaknetError::ReadPacketBufferError),
        }
    }
    pub(crate) fn read_slice(&mut self, length: usize) -> Result<&'a [u8]> {
        let start = self.buf.position() as usize;
        let end = start
            .checked_add(length)
            .ok_or(RaknetError::ReadPacketBufferError)?;
        let source = *self.buf.get_ref();
        let bytes = source
            .get(start..end)
            .ok_or(RaknetError::ReadPacketBufferError)?;
        self.buf.set_position(end as u64);
        Ok(bytes)
    }

    pub fn read_u8(&mut self) -> Result<u8> {
        if !self.buf.has_remaining() {
            return Err(RaknetError::ReadPacketBufferError);
        }
        Ok(self.buf.get_u8())
    }

    pub fn read_u16(&mut self, n: Endian) -> Result<u16> {
        if self.buf.remaining() < 2 {
            return Err(RaknetError::ReadPacketBufferError);
        }

        match n {
            Endian::Big => Ok(self.buf.get_u16()),
            Endian::Little => Ok(self.buf.get_u16_le()),
        }
    }

    pub fn read_u24(&mut self, n: Endian) -> Result<u32> {
        if self.buf.remaining() < 3 {
            return Err(RaknetError::ReadPacketBufferError);
        }

        match n {
            Endian::Big => {
                let a = self.buf.get_u8();
                let b = self.buf.get_u8();
                let c = self.buf.get_u8();

                let ret = u32::from_be_bytes([0, a, b, c]);
                Ok(ret)
            }
            Endian::Little => {
                let a = self.buf.get_u8();
                let b = self.buf.get_u8();
                let c = self.buf.get_u8();

                let ret = u32::from_le_bytes([a, b, c, 0]);
                Ok(ret)
            }
        }
    }

    pub fn read_u32(&mut self, n: Endian) -> Result<u32> {
        if self.buf.remaining() < 4 {
            return Err(RaknetError::ReadPacketBufferError);
        }

        match n {
            Endian::Big => Ok(self.buf.get_u32()),
            Endian::Little => Ok(self.buf.get_u32_le()),
        }
    }

    pub fn read_u64(&mut self, n: Endian) -> Result<u64> {
        if self.buf.remaining() < 8 {
            return Err(RaknetError::ReadPacketBufferError);
        }

        match n {
            Endian::Big => Ok(self.buf.get_u64()),
            Endian::Little => Ok(self.buf.get_u64_le()),
        }
    }
    pub fn read_i64(&mut self, n: Endian) -> Result<i64> {
        if self.buf.remaining() < 8 {
            return Err(RaknetError::ReadPacketBufferError);
        }

        match n {
            Endian::Big => Ok(self.buf.get_i64()),
            Endian::Little => Ok(self.buf.get_i64_le()),
        }
    }

    pub fn read_string(&mut self) -> Result<String> {
        if self.buf.remaining() < 2 {
            return Err(RaknetError::ReadPacketBufferError);
        }

        let size = usize::from(self.read_u16(Endian::Big)?);
        let mut buf = vec![0; size];
        self.read(&mut buf)?;
        String::from_utf8(buf).map_err(|_| RaknetError::PacketParseError)
    }

    pub fn read_magic(&mut self) -> Result<bool> {
        if self.buf.remaining() < 16 {
            return Err(RaknetError::ReadPacketBufferError);
        }

        let mut magic = [0; 16];
        self.read(&mut magic)?;
        let offline_magic = [
            0x00, 0xff, 0xff, 0x00, 0xfe, 0xfe, 0xfe, 0xfe, 0xfd, 0xfd, 0xfd, 0xfd, 0x12, 0x34,
            0x56, 0x78,
        ];
        Ok(magic == offline_magic)
    }

    pub fn read_address(&mut self) -> Result<SocketAddr> {
        let ip_ver = self.read_u8()?;

        if ip_ver == 4 {
            if self.buf.remaining() < 6 {
                return Err(RaknetError::ReadPacketBufferError);
            }

            let ip = Ipv4Addr::new(
                0xff - self.read_u8()?,
                0xff - self.read_u8()?,
                0xff - self.read_u8()?,
                0xff - self.read_u8()?,
            );
            let port = self.read_u16(Endian::Big)?;
            Ok(SocketAddr::new(IpAddr::V4(ip), port))
        } else if ip_ver == 6 || ip_ver == 23 {
            // Standard IPv6 addresses include a version byte before AF_INET6.
            // Accept the legacy encoding emitted by releases through 0.14.2.
            if ip_ver == 6 {
                // AF_INET6 is native to the sender's OS (10 on Linux, 23 on
                // Windows). The explicit IP version determines the wire layout.
                self.skip(2)?;
            } else {
                self.skip(1)?;
            }
            let port = self.read_u16(Endian::Big)?;
            self.skip(4)?;
            let mut address_bytes = [0; 16];
            self.read(&mut address_bytes)?;
            self.skip(4)?;
            Ok(SocketAddr::new(
                IpAddr::V6(Ipv6Addr::from(address_bytes)),
                port,
            ))
        } else {
            Err(RaknetError::PacketHeaderError)
        }
    }

    pub fn skip(&mut self, n: usize) -> Result<()> {
        if self.buf.remaining() < n {
            return Err(RaknetError::ReadPacketBufferError);
        }
        self.buf.advance(n);
        Ok(())
    }

    pub fn pos(&self) -> u64 {
        self.buf.position()
    }
}

#[test]
fn truncated_reads_return_errors() {
    let mut reader = RaknetReader::new(&[]);
    assert!(matches!(
        reader.read_u8(),
        Err(RaknetError::ReadPacketBufferError)
    ));
}

#[test]
fn invalid_utf8_returns_a_parse_error() {
    let bytes = [0, 1, 0xff];
    let mut reader = RaknetReader::new(&bytes);
    assert!(matches!(
        reader.read_string(),
        Err(RaknetError::PacketParseError)
    ));
}

#[test]
fn strings_longer_than_the_wire_length_are_rejected() {
    let mut writer = RaknetWriter::new();
    assert!(matches!(
        writer.write_string(&"x".repeat(usize::from(u16::MAX) + 1)),
        Err(RaknetError::PacketParseError)
    ));
}

#[test]
fn socket_addresses_round_trip() {
    for address in [
        "127.0.0.1:19132".parse().unwrap(),
        "[::1]:19132".parse().unwrap(),
    ] {
        let mut writer = RaknetWriter::new();
        writer.write_address(address).unwrap();
        let bytes = writer.get_raw_payload();
        let mut reader = RaknetReader::new(&bytes);
        assert_eq!(reader.read_address().unwrap(), address);
    }
}

#[tokio::test]
async fn test_u24_encode_decode() {
    let a: u32 = 65535 * 21;
    let b = a.to_le_bytes();
    let mut reader = RaknetReader::new(&b);

    let c = reader.read_u24(Endian::Little).unwrap();

    assert!(a == c);

    let mut writer = RaknetWriter::new();
    writer.write_u24(a, Endian::Little).unwrap();

    let buf = writer.get_raw_payload();
    let mut reader = RaknetReader::new(&buf);

    let c = reader.read_u24(Endian::Little).unwrap();

    assert!(a == c);
}

#[test]
fn ipv6_address_matches_the_raknet_wire_format() {
    let address = "[::1]:19132".parse().unwrap();
    let mut writer = RaknetWriter::new();
    writer.write_address(address).unwrap();
    let bytes = writer.get_raw_payload();
    assert_eq!(bytes.len(), 29);
    assert_eq!(&bytes[..9], &[6, 23, 0, 0x4a, 0xbc, 0, 0, 0, 0]);
    assert_eq!(bytes[24], 1);
    assert_eq!(RaknetReader::new(&bytes).read_address().unwrap(), address);
    assert_eq!(
        RaknetReader::new(&bytes[1..]).read_address().unwrap(),
        address
    );
}

#[test]
fn ipv6_addresses_accept_native_family_values_from_other_platforms() {
    let address = "[::1]:19132".parse().unwrap();
    let mut writer = RaknetWriter::new();
    writer.write_address(address).unwrap();
    let mut bytes = writer.get_raw_payload();
    for family in [10_u16, 23, 28, 30] {
        bytes[1..3].copy_from_slice(&family.to_le_bytes());
        assert_eq!(RaknetReader::new(&bytes).read_address().unwrap(), address);
    }
    for length in 0..bytes.len() {
        assert!(RaknetReader::new(&bytes[..length]).read_address().is_err());
    }
}

#[test]
fn borrowed_reads_validate_lengths_without_consuming_truncated_payloads() {
    let bytes = [1, 2, 3];
    let mut reader = RaknetReader::new(&bytes);
    assert_eq!(reader.read_u8().unwrap(), 1);
    assert!(reader.read_slice(3).is_err());
    assert!(reader.read_slice(usize::MAX).is_err());
    assert_eq!(reader.remaining(), 2);
    assert_eq!(reader.read_slice(2).unwrap(), &[2, 3]);
    assert_eq!(reader.read_slice(0).unwrap(), &[]);
    assert!(reader.read_slice(1).is_err());
}
