use crate::error::*;
use crate::utils::Endian;
use bytes::{Buf, BufMut};
use std::{
    io::{Cursor, Read},
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
    str,
};

#[derive(Clone)]
pub struct RaknetWriter {
    buf: Vec<u8>,
}

impl RaknetWriter {
    pub fn new() -> Self {
        Self { buf: vec![] }
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
        self.buf.put_u16(raw.len() as u16);
        self.buf.put_slice(raw);
        Ok(())
    }

    pub fn write_address(&mut self, address: SocketAddr) -> Result<()> {
        if address.is_ipv4() {
            self.write_u8(0x4)?;
            let ip_bytes = match address.ip() {
                IpAddr::V4(ip) => ip.octets().to_vec(),
                _ => vec![0; 4],
            };

            self.write_u8(0xff - ip_bytes[0])?;
            self.write_u8(0xff - ip_bytes[1])?;
            self.write_u8(0xff - ip_bytes[2])?;
            self.write_u8(0xff - ip_bytes[3])?;
            self.write_u16(address.port(), Endian::Big)?;
            Ok(())
        } else {
            self.write_i16(23, Endian::Little)?;
            self.write_u16(address.port(), Endian::Big)?;
            self.write_i32(0, Endian::Big)?;
            let ip_bytes = match address.ip() {
                IpAddr::V6(ip) => ip.octets().to_vec(),
                _ => vec![0; 16],
            };
            self.write(&ip_bytes)?;
            self.write_i32(0, Endian::Big)?;
            Ok(())
        }
    }

    pub fn get_raw_payload(self) -> Vec<u8> {
        self.buf
    }

    pub fn _pos(&self) -> u64 {
        self.buf.len() as u64
    }
}

pub struct RaknetReader {
    buf: Cursor<Vec<u8>>,
}

impl RaknetReader {
    pub fn new(buf: Vec<u8>) -> Self {
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
    pub fn read_u8(&mut self) -> Result<u8> {
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

        let size = self.read_u16(Endian::Big)?;
        let mut buf = vec![0u8; size as usize].into_boxed_slice();

        if self.buf.remaining() < size as usize {
            return Err(RaknetError::ReadPacketBufferError);
        }

        self.read(&mut buf)?;
        Ok(String::from_utf8(buf.to_vec()).unwrap())
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
        } else {
            if self.buf.remaining() < 44 {
                return Err(RaknetError::ReadPacketBufferError);
            }

            self.next(2);
            let port = self.read_u16(Endian::Big)?;
            self.next(4);
            let mut addr_buf = [0; 16];
            self.read(&mut addr_buf)?;

            let mut address_cursor = RaknetReader::new(addr_buf.to_vec());
            self.next(4);
            Ok(SocketAddr::new(
                IpAddr::V6(Ipv6Addr::new(
                    address_cursor.read_u16(Endian::Big)?,
                    address_cursor.read_u16(Endian::Big)?,
                    address_cursor.read_u16(Endian::Big)?,
                    address_cursor.read_u16(Endian::Big)?,
                    address_cursor.read_u16(Endian::Big)?,
                    address_cursor.read_u16(Endian::Big)?,
                    address_cursor.read_u16(Endian::Big)?,
                    address_cursor.read_u16(Endian::Big)?,
                )),
                port,
            ))
        } //IPv6 address = 128bit = u8 * 16
    }

    pub fn next(&mut self, n: u64) {
        self.buf.set_position(self.buf.position() + n);
    }

    pub fn pos(&self) -> u64 {
        self.buf.position()
    }
}

#[tokio::test]
async fn test_u24_encode_decode() {
    let a: u32 = 65535 * 21;
    let b = a.to_le_bytes();
    let mut reader = RaknetReader::new(b.to_vec());

    let c = reader.read_u24(Endian::Little).unwrap();

    assert!(a == c);

    let mut writer = RaknetWriter::new();
    writer.write_u24(a, Endian::Little).unwrap();

    let buf = writer.get_raw_payload();
    let mut reader = RaknetReader::new(buf);

    let c = reader.read_u24(Endian::Little).unwrap();

    assert!(a == c);
}
