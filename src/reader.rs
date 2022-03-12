use std::{
    io::{Cursor, Result},
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
    str,
};
use tokio::io::AsyncReadExt;
use tokio_byteorder::{AsyncReadBytesExt, BigEndian, LittleEndian};

pub enum Endian {
    Big,
    Little,
}

#[derive(Clone)]
pub struct Reader<'a> {
    cursor: Cursor<&'a [u8]>,
    strbuf: Vec<u8>,
}

impl<'a> Reader<'a> {
    pub fn new(buf: &'a [u8]) -> Self {
        Self {
            cursor: Cursor::new(buf),
            strbuf: Vec::with_capacity(0),
        }
    }
    pub async fn read(&mut self, buf: &mut [u8]) -> Result<()> {
        self.cursor.read_exact(buf).await?;
        Ok(())
    }
    pub async fn read_u8(&mut self) -> Result<u8> {
        AsyncReadBytesExt::read_u8(&mut self.cursor).await
    }

    pub async fn read_u16(&mut self, n: Endian) -> Result<u16> {
        match n {
            Endian::Big => AsyncReadBytesExt::read_u16::<BigEndian>(&mut self.cursor).await,
            Endian::Little => AsyncReadBytesExt::read_u16::<LittleEndian>(&mut self.cursor).await,
        }
    }

    pub async fn read_u32(&mut self, n: Endian) -> Result<u32> {
        match n {
            Endian::Big => AsyncReadBytesExt::read_u32::<BigEndian>(&mut self.cursor).await,
            Endian::Little => AsyncReadBytesExt::read_u32::<LittleEndian>(&mut self.cursor).await,
        }
    }

    pub async fn read_u64(&mut self, n: Endian) -> Result<u64> {
        match n {
            Endian::Big => AsyncReadBytesExt::read_u64::<BigEndian>(&mut self.cursor).await,
            Endian::Little => AsyncReadBytesExt::read_u64::<LittleEndian>(&mut self.cursor).await,
        }
    }
    pub async fn read_i64(&mut self, n: Endian) -> Result<i64> {
        match n {
            Endian::Big => AsyncReadBytesExt::read_i64::<BigEndian>(&mut self.cursor).await,
            Endian::Little => AsyncReadBytesExt::read_i64::<LittleEndian>(&mut self.cursor).await,
        }
    }

    pub async fn read_u24(&mut self, n: Endian) -> Result<u32> {
        match n {
            Endian::Big => self.cursor.read_u24::<BigEndian>().await,
            Endian::Little => self.cursor.read_u24::<LittleEndian>().await,
        }
    }

    pub async fn read_string(&'a mut self) -> Result<&'a str> {
        let size = self.read_u16(Endian::Big).await?;
        self.strbuf.resize(size.into(), 0);
        assert!(self.cursor.read_exact(&mut self.strbuf).await? == size.into());
        Ok(str::from_utf8(&self.strbuf).unwrap())
    }
    pub async fn read_magic(&mut self) -> Result<bool> {
        let mut magic = [0; 16];
        self.cursor.read_exact(&mut magic).await?;
        let offline_magic = [
            0x00, 0xff, 0xff, 0x00, 0xfe, 0xfe, 0xfe, 0xfe, 0xfd, 0xfd, 0xfd, 0xfd, 0x12, 0x34,
            0x56, 0x78,
        ];
        Ok(magic == offline_magic)
    }
    pub async fn read_address(&mut self) -> Result<SocketAddr> {
        let ip_ver = self.read_u8().await?;

        if ip_ver == 4 {
            let ip = Ipv4Addr::new(
                0xff - self.read_u8().await?,
                0xff - self.read_u8().await?,
                0xff - self.read_u8().await?,
                0xff - self.read_u8().await?,
            );
            let port = AsyncReadBytesExt::read_u16::<BigEndian>(&mut self.cursor).await?;
            Ok(SocketAddr::new(IpAddr::V4(ip), port))
        } else {
            self.next(2);
            let port = AsyncReadBytesExt::read_u16::<LittleEndian>(&mut self.cursor).await?;
            self.next(4);
            let mut addr_buf = [0; 16];
            self.cursor.read_exact(&mut addr_buf).await?;

            let mut address_cursor = Reader::new(&addr_buf);
            self.next(4);
            Ok(SocketAddr::new(
                IpAddr::V6(Ipv6Addr::new(
                    address_cursor.read_u16(Endian::Big).await?,
                    address_cursor.read_u16(Endian::Big).await?,
                    address_cursor.read_u16(Endian::Big).await?,
                    address_cursor.read_u16(Endian::Big).await?,
                    address_cursor.read_u16(Endian::Big).await?,
                    address_cursor.read_u16(Endian::Big).await?,
                    address_cursor.read_u16(Endian::Big).await?,
                    address_cursor.read_u16(Endian::Big).await?,
                )),
                port,
            ))
        } //IPv6 address = 128bit = u8 * 16
    }
    pub fn next(&mut self, n: u64) {
        self.cursor.set_position(self.cursor.position() + n);
    }

    pub fn pos(&self) -> u64 {
        self.cursor.position()
    }
}
