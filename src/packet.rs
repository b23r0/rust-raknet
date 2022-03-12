use std::io::Result;
use crate::reader::{Reader, Endian};
use crate::writer::{Writer};

#[warn(non_camel_case_types)]
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum PacketID {
    UnconnectedPing1 = 0x01,
    UnconnectedPing2 = 0x02,
    UnconnectedPong = 0x1c,
    OpenConnectionRequest1 = 0x05,
    OpenConnectionReply1 = 0x06,
    OpenConnectionRequest2 = 0x07,
    OpenConnectionReply2 = 0x08,
    Unknown = 0xff
}

pub fn transaction_packet_id(id : u8) -> PacketID {
    match id{
        0x01 => PacketID::UnconnectedPing1,
        0x02 => PacketID::UnconnectedPing2,
        0x1c => PacketID::UnconnectedPong,
        0x05 => PacketID::OpenConnectionRequest1,
        0x06 => PacketID::OpenConnectionReply1,
        0x07 => PacketID::OpenConnectionRequest2,
        0x08 => PacketID::OpenConnectionReply2,
        _ => PacketID::Unknown
    }
}

pub fn transaction_packet_id_to_u8(packetid : PacketID) -> u8 {
    match packetid{
        PacketID::UnconnectedPing1 => 0x01,
        PacketID::UnconnectedPing2 => 0x02,
        PacketID::UnconnectedPong => 0x1c,
        PacketID::OpenConnectionRequest1 => 0x05,
        PacketID::OpenConnectionReply1 => 0x06,
        PacketID::OpenConnectionRequest2 => 0x07,
        PacketID::OpenConnectionReply2 => 0x08,
        PacketID::Unknown => 0xff,
    }
}

macro_rules! unwrap_or_return {
    ($res:expr) => {
        match $res.await {
            Ok(val) => val,
            Err(e) => {
                return Err(e);
            }
        }
    };
}

#[derive(Clone)]
pub struct PacketUnconnectedPing {
    pub time: i64,
    pub magic: bool,
    pub guid: u64,
}

#[derive(Clone)]
pub struct PacketUnconnectedPong {
    pub time: i64,
    pub guid: u64,
    pub magic: bool,
    pub motd: String,
}

pub async fn read_packet_ping(buf : &[u8]) -> Result<PacketUnconnectedPing>{
    let mut cursor = Reader::new(buf);
    unwrap_or_return!(cursor.read_u8());
    Ok(PacketUnconnectedPing {
        time: unwrap_or_return!(cursor.read_i64(Endian::Big)),
        magic: unwrap_or_return!(cursor.read_magic()),
        guid: unwrap_or_return!(cursor.read_u64(Endian::Big)),
    })
}

pub async fn write_packet_ping(packet : &PacketUnconnectedPing) -> Result<Vec<u8>>{
    let mut cursor = Writer::new(vec![]);
    unwrap_or_return!(cursor.write_u8(transaction_packet_id_to_u8(PacketID::UnconnectedPing1)));
    unwrap_or_return!(cursor.write_i64(packet.time, Endian::Big));
    unwrap_or_return!(cursor.write_magic());
    unwrap_or_return!(cursor.write_u64(packet.guid, Endian::Big));
    Ok(cursor.get_raw_payload())
}

pub async fn read_packet_pong(buf : &[u8]) -> Result<PacketUnconnectedPong>{
    let mut cursor = Reader::new(buf);
    unwrap_or_return!(cursor.read_u8());
    Ok(PacketUnconnectedPong {
        time: unwrap_or_return!(cursor.read_i64(Endian::Big)),
        guid: unwrap_or_return!(cursor.read_u64(Endian::Big)),
        magic: unwrap_or_return!(cursor.read_magic()),
        motd: unwrap_or_return!(cursor.read_string()).to_owned(),
    })
}

pub async fn write_packet_pong(packet : &PacketUnconnectedPong) -> Result<Vec<u8>>{
    let mut cursor = Writer::new(vec![]);
    unwrap_or_return!(cursor.write_u8(transaction_packet_id_to_u8(PacketID::UnconnectedPong)));
    unwrap_or_return!(cursor.write_i64(packet.time, Endian::Big));
    unwrap_or_return!(cursor.write_u64(packet.guid, Endian::Big));
    unwrap_or_return!(cursor.write_magic());
    unwrap_or_return!(cursor.write_string(&packet.motd));
    Ok(cursor.get_raw_payload())
}