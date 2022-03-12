use crate::reader::Endian;
use std::{
    io::{Cursor, Result},
    net::{IpAddr, SocketAddr},
    str,
};
use tokio::io::AsyncWriteExt;
use tokio_byteorder::{AsyncWriteBytesExt, BigEndian, LittleEndian};

#[derive(Clone)]
pub struct Writer {
    cursor: Cursor<Vec<u8>>,
}

impl Writer {
    pub fn new(buf: Vec<u8>) -> Self {
        Self {
            cursor: Cursor::new(buf),
        }
    }
    pub async fn write(&mut self, v: &[u8]) -> Result<()> {
        self.cursor.write_all(v).await
    }

    pub async fn write_u8(&mut self, v: u8) -> Result<()> {
        AsyncWriteBytesExt::write_u8(&mut self.cursor, v).await
    }

    pub async fn write_u16(&mut self, v: u16, n: Endian) -> Result<()> {
        match n {
            Endian::Big => AsyncWriteBytesExt::write_u16::<BigEndian>(&mut self.cursor, v).await,
            Endian::Little => {
                AsyncWriteBytesExt::write_u16::<LittleEndian>(&mut self.cursor, v).await
            }
        }
    }
    pub async fn write_u32(&mut self, v: u32, n: Endian) -> Result<()> {
        match n {
            Endian::Big => AsyncWriteBytesExt::write_u32::<BigEndian>(&mut self.cursor, v).await,
            Endian::Little => {
                AsyncWriteBytesExt::write_u32::<LittleEndian>(&mut self.cursor, v).await
            }
        }
    }
    pub async fn write_u24(&mut self, v: u32, n: Endian) -> Result<()> {
        match n {
            Endian::Big => self.cursor.write_u24::<BigEndian>(v).await,
            Endian::Little => self.cursor.write_u24::<LittleEndian>(v).await,
        }
    }

    pub async fn write_u64(&mut self, v: u64, n: Endian) -> Result<()> {
        match n {
            Endian::Big => AsyncWriteBytesExt::write_u64::<BigEndian>(&mut self.cursor, v).await,
            Endian::Little => {
                AsyncWriteBytesExt::write_u64::<LittleEndian>(&mut self.cursor, v).await
            }
        }
    }

    pub async fn write_i64(&mut self, v: i64, n: Endian) -> Result<()> {
        match n {
            Endian::Big => AsyncWriteBytesExt::write_i64::<BigEndian>(&mut self.cursor, v).await,
            Endian::Little => {
                AsyncWriteBytesExt::write_i64::<LittleEndian>(&mut self.cursor, v).await
            }
        }
    }

    pub async fn write_string(&mut self, body: &str) -> Result<()> {
        let raw = body.as_bytes();
        self.write_u16(raw.len() as u16, Endian::Big).await?;
        self.write(raw).await
    }
    pub async fn write_magic(&mut self) -> Result<usize> {
        let magic = [
            0x00, 0xff, 0xff, 0x00, 0xfe, 0xfe, 0xfe, 0xfe, 0xfd, 0xfd, 0xfd, 0xfd, 0x12, 0x34,
            0x56, 0x78,
        ];
        self.cursor.write(&magic).await
    }
    pub async fn write_address(&mut self, address: SocketAddr) -> Result<()> {
        if address.is_ipv4() {
            AsyncWriteBytesExt::write_u8(&mut self.cursor, 0x4).await?;
            let ip_bytes = match address.ip() {
                IpAddr::V4(ip) => ip.octets().to_vec(),
                _ => vec![0; 4],
            };

            self.write_u8(0xff - ip_bytes[0]).await?;
            self.write_u8(0xff - ip_bytes[1]).await?;
            self.write_u8(0xff - ip_bytes[2]).await?;
            self.write_u8(0xff - ip_bytes[3]).await?;
            AsyncWriteBytesExt::write_u16::<BigEndian>(&mut self.cursor, address.port()).await?;
            Ok(())
        } else {
            AsyncWriteBytesExt::write_i16::<LittleEndian>(&mut self.cursor, 23).await?;
            AsyncWriteBytesExt::write_u16::<BigEndian>(&mut self.cursor, address.port()).await?;
            AsyncWriteBytesExt::write_i32::<BigEndian>(&mut self.cursor, 0).await?;
            let ip_bytes = match address.ip() {
                IpAddr::V6(ip) => ip.octets().to_vec(),
                _ => vec![0; 16],
            };
            self.write(&ip_bytes).await?;
            AsyncWriteBytesExt::write_i32::<BigEndian>(&mut self.cursor, 0).await?;
            Ok(())
        }
    }

    pub fn get_raw_payload(self) -> Vec<u8> {
        self.cursor.into_inner()
    }

    pub fn pos(&self) -> u64 {
        self.cursor.position()
    }
}
