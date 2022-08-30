use crate::datatype::{RaknetReader, RaknetWriter};
use crate::error::RaknetError;
use crate::error::*;
use crate::utils::Endian;
use std::net::SocketAddr;

#[warn(non_camel_case_types)]
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum PacketID {
    ConnectedPing = 0x00,
    UnconnectedPing1 = 0x01,
    UnconnectedPing2 = 0x02,
    ConnectedPong = 0x03,
    UnconnectedPong = 0x1c,
    OpenConnectionRequest1 = 0x05,
    OpenConnectionReply1 = 0x06,
    OpenConnectionRequest2 = 0x07,
    OpenConnectionReply2 = 0x08,
    ConnectionRequest = 0x09,
    ConnectionRequestAccepted = 0x10,
    AlreadyConnected = 0x12,
    NewIncomingConnection = 0x13,
    Disconnect = 0x15,
    IncompatibleProtocolVersion = 0x19,
    FrameSetPacketBegin = 0x80,
    FrameSetPacketEnd = 0x8d,
    Nack = 0xa0,
    Ack = 0xc0,
    Game = 0xfe,
}

impl PacketID {
    pub fn to_u8(self) -> u8 {
        match self {
            PacketID::ConnectedPing => 0x00,
            PacketID::UnconnectedPing1 => 0x01,
            PacketID::UnconnectedPing2 => 0x02,
            PacketID::ConnectedPong => 0x03,
            PacketID::UnconnectedPong => 0x1c,
            PacketID::OpenConnectionRequest1 => 0x05,
            PacketID::OpenConnectionReply1 => 0x06,
            PacketID::OpenConnectionRequest2 => 0x07,
            PacketID::OpenConnectionReply2 => 0x08,
            PacketID::ConnectionRequest => 0x09,
            PacketID::ConnectionRequestAccepted => 0x10,
            PacketID::AlreadyConnected => 0x12,
            PacketID::NewIncomingConnection => 0x13,
            PacketID::Disconnect => 0x15,
            PacketID::IncompatibleProtocolVersion => 0x19,
            PacketID::FrameSetPacketBegin => 0x80,
            PacketID::FrameSetPacketEnd => 0x8d,
            PacketID::Nack => 0xa0,
            PacketID::Ack => 0xc0,
            PacketID::Game => 0xfe,
        }
    }

    pub fn from(id: u8) -> Result<Self> {
        if (0x80..=0x8d).contains(&id) {
            return Ok(PacketID::FrameSetPacketBegin);
        }

        match id {
            0x00 => Ok(PacketID::ConnectedPing),
            0x01 => Ok(PacketID::UnconnectedPing1),
            0x02 => Ok(PacketID::UnconnectedPing2),
            0x03 => Ok(PacketID::ConnectedPong),
            0x1c => Ok(PacketID::UnconnectedPong),
            0x05 => Ok(PacketID::OpenConnectionRequest1),
            0x06 => Ok(PacketID::OpenConnectionReply1),
            0x07 => Ok(PacketID::OpenConnectionRequest2),
            0x08 => Ok(PacketID::OpenConnectionReply2),
            0x09 => Ok(PacketID::ConnectionRequest),
            0x10 => Ok(PacketID::ConnectionRequestAccepted),
            0x12 => Ok(PacketID::AlreadyConnected),
            0x13 => Ok(PacketID::NewIncomingConnection),
            0x15 => Ok(PacketID::Disconnect),
            0x19 => Ok(PacketID::IncompatibleProtocolVersion),
            0x80 => Ok(PacketID::FrameSetPacketBegin),
            0x8d => Ok(PacketID::FrameSetPacketEnd),
            0xa0 => Ok(PacketID::Nack),
            0xc0 => Ok(PacketID::Ack),
            0xfe => Ok(PacketID::Game),
            _ => Err(RaknetError::IncorrectPacketID),
        }
    }
}

macro_rules! unwrap_or_return {
    ($res:expr) => {
        match $res {
            Ok(val) => val,
            Err(e) => {
                return Err(e);
            }
        }
    };
}

#[derive(Clone)]
pub struct ConnectedPing {
    pub client_timestamp: i64,
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

#[derive(Clone)]
pub struct ConnectedPong {
    pub client_timestamp: i64,
    pub server_timestamp: i64,
}

#[derive(Clone)]
pub struct OpenConnectionRequest1 {
    pub magic: bool,
    pub protocol_version: u8,
    pub mtu_size: u16,
}

#[derive(Clone)]
pub struct OpenConnectionRequest2 {
    pub magic: bool,
    pub address: std::net::SocketAddr,
    pub mtu: u16,
    pub guid: u64,
}

#[derive(Clone)]
pub struct OpenConnectionReply1 {
    pub magic: bool,
    pub guid: u64,
    pub use_encryption: u8,
    pub mtu_size: u16,
}

#[derive(Clone)]
pub struct OpenConnectionReply2 {
    pub magic: bool,
    pub guid: u64,
    pub address: std::net::SocketAddr,
    pub mtu: u16,
    pub encryption_enabled: u8,
}

#[derive(Clone)]
pub struct ConnectionRequest {
    pub guid: u64,
    pub time: i64,
    pub use_encryption: u8,
}

#[derive(Clone)]
pub struct ConnectionRequestAccepted {
    pub client_address: std::net::SocketAddr,
    pub system_index: u16,
    pub request_timestamp: i64,
    pub accepted_timestamp: i64,
}

#[derive(Clone)]
pub struct NewIncomingConnection {
    pub server_address: std::net::SocketAddr,
    pub request_timestamp: i64,
    pub accepted_timestamp: i64,
}

#[derive(Clone)]
pub struct IncompatibleProtocolVersion {
    pub server_protocol: u8,
    pub magic: bool,
    pub server_guid: u64,
}

#[derive(Clone)]
pub struct AlreadyConnected {
    pub magic: bool,
    pub guid: u64,
}

#[derive(Clone)]
pub struct Nack {
    pub record_count: u16,
    pub sequences: Vec<(u32, u32)>,
}

#[derive(Clone)]
pub struct Ack {
    pub record_count: u16,
    pub sequences: Vec<(u32, u32)>,
}

pub fn read_packet_ping(buf: &[u8]) -> Result<PacketUnconnectedPing> {
    let mut cursor = RaknetReader::new(buf.to_vec());
    unwrap_or_return!(cursor.read_u8());
    Ok(PacketUnconnectedPing {
        time: unwrap_or_return!(cursor.read_i64(Endian::Big)),
        magic: unwrap_or_return!(cursor.read_magic()),
        guid: unwrap_or_return!(cursor.read_u64(Endian::Big)),
    })
}

pub fn write_packet_ping(packet: &PacketUnconnectedPing) -> Result<Vec<u8>> {
    let mut cursor = RaknetWriter::new();
    unwrap_or_return!(cursor.write_u8(PacketID::UnconnectedPing1.to_u8()));
    unwrap_or_return!(cursor.write_i64(packet.time, Endian::Big));
    unwrap_or_return!(cursor.write_magic());
    unwrap_or_return!(cursor.write_u64(packet.guid, Endian::Big));
    Ok(cursor.get_raw_payload())
}

pub fn read_packet_pong(buf: &[u8]) -> Result<PacketUnconnectedPong> {
    let mut cursor = RaknetReader::new(buf.to_vec());
    unwrap_or_return!(cursor.read_u8());
    Ok(PacketUnconnectedPong {
        time: unwrap_or_return!(cursor.read_i64(Endian::Big)),
        guid: unwrap_or_return!(cursor.read_u64(Endian::Big)),
        magic: unwrap_or_return!(cursor.read_magic()),
        motd: unwrap_or_return!(cursor.read_string()),
    })
}

pub fn write_packet_pong(packet: &PacketUnconnectedPong) -> Result<Vec<u8>> {
    let mut cursor = RaknetWriter::new();
    unwrap_or_return!(cursor.write_u8(PacketID::UnconnectedPong.to_u8()));
    unwrap_or_return!(cursor.write_i64(packet.time, Endian::Big));
    unwrap_or_return!(cursor.write_u64(packet.guid, Endian::Big));
    unwrap_or_return!(cursor.write_magic());
    unwrap_or_return!(cursor.write_string(&packet.motd));
    Ok(cursor.get_raw_payload())
}

pub fn read_packet_connection_open_request_1(buf: &[u8]) -> Result<OpenConnectionRequest1> {
    let mut cursor = RaknetReader::new(buf.to_vec());
    unwrap_or_return!(cursor.read_u8());
    Ok(OpenConnectionRequest1 {
        magic: unwrap_or_return!(cursor.read_magic()),
        protocol_version: unwrap_or_return!(cursor.read_u8()),
        //28 udp overhead
        //1492 - 46 +18 + 28 == 1492
        mtu_size: (buf.len() + 28).try_into().unwrap(),
    })
}

pub fn write_packet_connection_open_request_1(packet: &OpenConnectionRequest1) -> Result<Vec<u8>> {
    let mut cursor = RaknetWriter::new();
    unwrap_or_return!(cursor.write_u8(PacketID::OpenConnectionRequest1.to_u8()));
    unwrap_or_return!(cursor.write_magic());
    unwrap_or_return!(cursor.write_u8(packet.protocol_version));
    //The MTU sent in the response appears to be somewhere around the size of this padding + 46 (28 udp overhead, 1 packet id, 16 magic, 1 protocol version). This padding seems to be used to discover the maximum packet size the network can handle.
    unwrap_or_return!(cursor.write(vec![0; (packet.mtu_size as usize) - 46].as_slice()));

    Ok(cursor.get_raw_payload())
}

pub fn read_packet_connection_open_request_2(buf: &[u8]) -> Result<OpenConnectionRequest2> {
    let mut cursor = RaknetReader::new(buf.to_vec());
    unwrap_or_return!(cursor.read_u8());
    Ok(OpenConnectionRequest2 {
        magic: unwrap_or_return!(cursor.read_magic()),
        address: unwrap_or_return!(cursor.read_address()),
        mtu: unwrap_or_return!(cursor.read_u16(Endian::Big)),
        guid: unwrap_or_return!(cursor.read_u64(Endian::Big)),
    })
}

pub fn write_packet_connection_open_request_2(packet: &OpenConnectionRequest2) -> Result<Vec<u8>> {
    let mut cursor = RaknetWriter::new();
    unwrap_or_return!(cursor.write_u8(PacketID::OpenConnectionRequest2.to_u8()));
    unwrap_or_return!(cursor.write_magic());
    unwrap_or_return!(cursor.write_address(packet.address));
    unwrap_or_return!(cursor.write_u16(packet.mtu, Endian::Big));
    unwrap_or_return!(cursor.write_u64(packet.guid, Endian::Big));

    Ok(cursor.get_raw_payload())
}

pub fn read_packet_connection_open_reply_1(buf: &[u8]) -> Result<OpenConnectionReply1> {
    let mut cursor = RaknetReader::new(buf.to_vec());
    unwrap_or_return!(cursor.read_u8());
    Ok(OpenConnectionReply1 {
        magic: unwrap_or_return!(cursor.read_magic()),
        guid: unwrap_or_return!(cursor.read_u64(Endian::Big)),
        use_encryption: unwrap_or_return!(cursor.read_u8()),
        mtu_size: unwrap_or_return!(cursor.read_u16(Endian::Big)),
    })
}

pub fn write_packet_connection_open_reply_1(packet: &OpenConnectionReply1) -> Result<Vec<u8>> {
    let mut cursor = RaknetWriter::new();
    unwrap_or_return!(cursor.write_u8(PacketID::OpenConnectionReply1.to_u8()));
    unwrap_or_return!(cursor.write_magic());
    unwrap_or_return!(cursor.write_u64(packet.guid, Endian::Big));
    unwrap_or_return!(cursor.write_u8(packet.use_encryption));
    unwrap_or_return!(cursor.write_u16(packet.mtu_size, Endian::Big));

    Ok(cursor.get_raw_payload())
}

pub fn read_packet_connection_open_reply_2(buf: &[u8]) -> Result<OpenConnectionReply2> {
    let mut cursor = RaknetReader::new(buf.to_vec());
    unwrap_or_return!(cursor.read_u8());
    Ok(OpenConnectionReply2 {
        magic: unwrap_or_return!(cursor.read_magic()),
        guid: unwrap_or_return!(cursor.read_u64(Endian::Big)),
        address: unwrap_or_return!(cursor.read_address()),
        mtu: unwrap_or_return!(cursor.read_u16(Endian::Big)),
        encryption_enabled: unwrap_or_return!(cursor.read_u8()),
    })
}

pub fn write_packet_connection_open_reply_2(packet: &OpenConnectionReply2) -> Result<Vec<u8>> {
    let mut cursor = RaknetWriter::new();
    unwrap_or_return!(cursor.write_u8(PacketID::OpenConnectionReply2.to_u8()));
    unwrap_or_return!(cursor.write_magic());
    unwrap_or_return!(cursor.write_u64(packet.guid, Endian::Big));
    unwrap_or_return!(cursor.write_address(packet.address));
    unwrap_or_return!(cursor.write_u16(packet.mtu, Endian::Big));
    unwrap_or_return!(cursor.write_u8(packet.encryption_enabled));

    Ok(cursor.get_raw_payload())
}

pub fn _read_packet_already_connected(buf: &[u8]) -> Result<AlreadyConnected> {
    let mut cursor = RaknetReader::new(buf.to_vec());
    unwrap_or_return!(cursor.read_u8());
    Ok(AlreadyConnected {
        magic: unwrap_or_return!(cursor.read_magic()),
        guid: unwrap_or_return!(cursor.read_u64(Endian::Big)),
    })
}

pub fn write_packet_already_connected(packet: &AlreadyConnected) -> Result<Vec<u8>> {
    let mut cursor = RaknetWriter::new();
    unwrap_or_return!(cursor.write_u8(PacketID::AlreadyConnected.to_u8()));
    unwrap_or_return!(cursor.write_magic());
    unwrap_or_return!(cursor.write_u64(packet.guid, Endian::Big));
    Ok(cursor.get_raw_payload())
}

pub fn read_packet_incompatible_protocol_version(
    buf: &[u8],
) -> Result<IncompatibleProtocolVersion> {
    let mut cursor = RaknetReader::new(buf.to_vec());
    unwrap_or_return!(cursor.read_u8());
    Ok(IncompatibleProtocolVersion {
        server_protocol: unwrap_or_return!(cursor.read_u8()),
        magic: unwrap_or_return!(cursor.read_magic()),
        server_guid: unwrap_or_return!(cursor.read_u64(Endian::Big)),
    })
}

pub fn write_packet_incompatible_protocol_version(
    packet: &IncompatibleProtocolVersion,
) -> Result<Vec<u8>> {
    let mut cursor = RaknetWriter::new();
    unwrap_or_return!(cursor.write_u8(PacketID::IncompatibleProtocolVersion.to_u8()));
    unwrap_or_return!(cursor.write_u8(packet.server_protocol));
    unwrap_or_return!(cursor.write_magic());
    unwrap_or_return!(cursor.write_u64(packet.server_guid, Endian::Big));

    Ok(cursor.get_raw_payload())
}

pub fn read_packet_nack(buf: &[u8]) -> Result<Nack> {
    let mut cursor = RaknetReader::new(buf.to_vec());
    unwrap_or_return!(cursor.read_u8());
    let record_count = unwrap_or_return!(cursor.read_u16(Endian::Big));
    let sequences = {
        let mut s = vec![];
        for _ in 0..record_count {
            let single_sequence_number = unwrap_or_return!(cursor.read_u8());
            let sequence = unwrap_or_return!(cursor.read_u24(Endian::Little));
            if single_sequence_number == 0x01 {
                s.push((sequence, sequence));
            } else {
                let sequence_max = unwrap_or_return!(cursor.read_u24(Endian::Little));
                s.push((sequence, sequence_max));
            }
        }
        s
    };

    Ok(Nack {
        record_count,
        sequences,
    })
}

pub fn write_packet_nack(packet: &Nack) -> Result<Vec<u8>> {
    let mut cursor = RaknetWriter::new();
    unwrap_or_return!(cursor.write_u8(PacketID::Nack.to_u8()));
    cursor.write_u16(packet.record_count, Endian::Big)?;

    for i in 0..packet.record_count {
        let single_sequence_number =
            if packet.sequences[i as usize].0 == packet.sequences[i as usize].1 {
                1u8
            } else {
                0u8
            };
        cursor.write_u8(single_sequence_number)?;
        cursor.write_u24(packet.sequences[i as usize].0, Endian::Little)?;
        if single_sequence_number == 0x00 {
            cursor.write_u24(packet.sequences[i as usize].1, Endian::Little)?;
        }
    }

    Ok(cursor.get_raw_payload())
}

pub fn read_packet_ack(buf: &[u8]) -> Result<Ack> {
    let mut cursor = RaknetReader::new(buf.to_vec());
    unwrap_or_return!(cursor.read_u8());
    let record_count = unwrap_or_return!(cursor.read_u16(Endian::Big));
    let sequences = {
        let mut s = vec![];
        for _ in 0..record_count {
            let single_sequence_number = unwrap_or_return!(cursor.read_u8());
            let sequence = unwrap_or_return!(cursor.read_u24(Endian::Little));
            if single_sequence_number == 0x01 {
                s.push((sequence, sequence));
            } else {
                let sequence_max = unwrap_or_return!(cursor.read_u24(Endian::Little));
                s.push((sequence, sequence_max));
            }
        }
        s
    };
    Ok(Ack {
        record_count,
        sequences,
    })
}

pub fn write_packet_ack(packet: &Ack) -> Result<Vec<u8>> {
    let mut cursor = RaknetWriter::new();
    unwrap_or_return!(cursor.write_u8(PacketID::Ack.to_u8()));
    unwrap_or_return!(cursor.write_u16(packet.record_count, Endian::Big));

    for i in 0..packet.record_count {
        let single_sequence_number =
            if packet.sequences[i as usize].0 == packet.sequences[i as usize].1 {
                1u8
            } else {
                0u8
            };
        unwrap_or_return!(cursor.write_u8(single_sequence_number as u8));
        unwrap_or_return!(cursor.write_u24(packet.sequences[i as usize].0, Endian::Little));
        if single_sequence_number == 0x00 {
            unwrap_or_return!(cursor.write_u24(packet.sequences[i as usize].1, Endian::Little));
        }
    }

    Ok(cursor.get_raw_payload())
}

pub fn read_packet_connection_request(buf: &[u8]) -> Result<ConnectionRequest> {
    let mut cursor = RaknetReader::new(buf.to_vec());
    unwrap_or_return!(cursor.read_u8());
    Ok(ConnectionRequest {
        guid: unwrap_or_return!(cursor.read_u64(Endian::Big)),
        time: unwrap_or_return!(cursor.read_i64(Endian::Big)),
        use_encryption: unwrap_or_return!(cursor.read_u8()),
    })
}

pub fn write_packet_connection_request(packet: &ConnectionRequest) -> Result<Vec<u8>> {
    let mut cursor = RaknetWriter::new();
    unwrap_or_return!(cursor.write_u8(PacketID::ConnectionRequest.to_u8()));
    unwrap_or_return!(cursor.write_u64(packet.guid, Endian::Big));
    unwrap_or_return!(cursor.write_i64(packet.time, Endian::Big));
    unwrap_or_return!(cursor.write_u8(packet.use_encryption));

    Ok(cursor.get_raw_payload())
}

pub fn read_packet_connection_request_accepted(buf: &[u8]) -> Result<ConnectionRequestAccepted> {
    let mut cursor = RaknetReader::new(buf.to_vec());
    unwrap_or_return!(cursor.read_u8());
    Ok(ConnectionRequestAccepted {
        client_address: unwrap_or_return!(cursor.read_address()),
        system_index: unwrap_or_return!(cursor.read_u16(Endian::Big)),
        // Some Raknet server implementations cannot determine the exact number of Internal IDs, which may cause parsing failures.
        // But in fact, we don't use this parameter in the current implementation. So don't parse the value of this field for now.
        request_timestamp: 0,
        accepted_timestamp: 0,
    })
}

pub fn write_packet_connection_request_accepted(
    packet: &ConnectionRequestAccepted,
) -> Result<Vec<u8>> {
    let mut cursor = RaknetWriter::new();
    unwrap_or_return!(cursor.write_u8(PacketID::ConnectionRequestAccepted.to_u8()));
    unwrap_or_return!(cursor.write_address(packet.client_address));
    unwrap_or_return!(cursor.write_u16(packet.system_index, Endian::Big));
    let tmp_address: SocketAddr = "255.255.255.255:19132".parse().unwrap();
    for _ in 0..10 {
        unwrap_or_return!(cursor.write_address(tmp_address));
    }
    unwrap_or_return!(cursor.write_i64(packet.request_timestamp, Endian::Big));
    unwrap_or_return!(cursor.write_i64(packet.accepted_timestamp, Endian::Big));

    Ok(cursor.get_raw_payload())
}

pub fn read_packet_new_incomming_connection(buf: &[u8]) -> Result<NewIncomingConnection> {
    let mut cursor = RaknetReader::new(buf.to_vec());
    unwrap_or_return!(cursor.read_u8());
    Ok(NewIncomingConnection {
        server_address: unwrap_or_return!(cursor.read_address()),
        request_timestamp: {
            for _ in 0..10 {
                unwrap_or_return!(cursor.read_address());
            }
            unwrap_or_return!(cursor.read_i64(Endian::Big))
        },
        accepted_timestamp: unwrap_or_return!(cursor.read_i64(Endian::Big)),
    })
}

pub fn write_packet_new_incomming_connection(packet: &NewIncomingConnection) -> Result<Vec<u8>> {
    let mut cursor = RaknetWriter::new();
    unwrap_or_return!(cursor.write_u8(PacketID::NewIncomingConnection.to_u8()));
    unwrap_or_return!(cursor.write_address(packet.server_address));
    let tmp_address: SocketAddr = "0.0.0.0:0".parse().unwrap();
    for _ in 0..10 {
        unwrap_or_return!(cursor.write_address(tmp_address));
    }
    unwrap_or_return!(cursor.write_i64(packet.request_timestamp, Endian::Big));
    unwrap_or_return!(cursor.write_i64(packet.accepted_timestamp, Endian::Big));

    Ok(cursor.get_raw_payload())
}

pub fn read_packet_connected_ping(buf: &[u8]) -> Result<ConnectedPing> {
    let mut cursor = RaknetReader::new(buf.to_vec());
    unwrap_or_return!(cursor.read_u8());
    Ok(ConnectedPing {
        client_timestamp: unwrap_or_return!(cursor.read_i64(Endian::Big)),
    })
}

pub fn write_packet_connected_ping(packet: &ConnectedPing) -> Result<Vec<u8>> {
    let mut cursor = RaknetWriter::new();
    unwrap_or_return!(cursor.write_u8(PacketID::ConnectedPing.to_u8()));
    unwrap_or_return!(cursor.write_i64(packet.client_timestamp, Endian::Big));

    Ok(cursor.get_raw_payload())
}

pub fn _read_packet_connected_pong(buf: &[u8]) -> Result<ConnectedPong> {
    let mut cursor = RaknetReader::new(buf.to_vec());
    unwrap_or_return!(cursor.read_u8());
    Ok(ConnectedPong {
        client_timestamp: unwrap_or_return!(cursor.read_i64(Endian::Big)),
        server_timestamp: unwrap_or_return!(cursor.read_i64(Endian::Big)),
    })
}

pub fn write_packet_connected_pong(packet: &ConnectedPong) -> Result<Vec<u8>> {
    let mut cursor = RaknetWriter::new();
    unwrap_or_return!(cursor.write_u8(PacketID::ConnectedPong.to_u8()));
    unwrap_or_return!(cursor.write_i64(packet.client_timestamp, Endian::Big));
    unwrap_or_return!(cursor.write_i64(packet.server_timestamp, Endian::Big));
    Ok(cursor.get_raw_payload())
}
