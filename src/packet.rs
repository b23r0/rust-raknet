use crate::datatype::{RaknetReader, RaknetWriter};
use crate::error::RaknetError;
use crate::error::*;
use crate::utils::Endian;
use std::net::SocketAddr;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
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
        self as u8
    }

    pub fn from(id: u8) -> Result<Self> {
        Self::try_from(id)
    }
}

impl TryFrom<u8> for PacketID {
    type Error = RaknetError;

    fn try_from(id: u8) -> Result<Self> {
        if (0x80..=0x8d).contains(&id) {
            return Ok(Self::FrameSetPacketBegin);
        }

        match id {
            0x00 => Ok(Self::ConnectedPing),
            0x01 => Ok(Self::UnconnectedPing1),
            0x02 => Ok(Self::UnconnectedPing2),
            0x03 => Ok(Self::ConnectedPong),
            0x1c => Ok(Self::UnconnectedPong),
            0x05 => Ok(Self::OpenConnectionRequest1),
            0x06 => Ok(Self::OpenConnectionReply1),
            0x07 => Ok(Self::OpenConnectionRequest2),
            0x08 => Ok(Self::OpenConnectionReply2),
            0x09 => Ok(Self::ConnectionRequest),
            0x10 => Ok(Self::ConnectionRequestAccepted),
            0x12 => Ok(Self::AlreadyConnected),
            0x13 => Ok(Self::NewIncomingConnection),
            0x15 => Ok(Self::Disconnect),
            0x19 => Ok(Self::IncompatibleProtocolVersion),
            0x80 => Ok(Self::FrameSetPacketBegin),
            0x8d => Ok(Self::FrameSetPacketEnd),
            0xa0 => Ok(Self::Nack),
            0xc0 => Ok(Self::Ack),
            0xfe => Ok(Self::Game),
            _ => Err(RaknetError::IncorrectPacketID),
        }
    }
}

#[derive(Clone)]
pub struct ConnectedPing {
    pub client_timestamp: i64,
}

#[derive(Clone)]
pub struct PacketUnconnectedPing {
    pub time: i64,
    pub guid: u64,
}

#[derive(Clone)]
pub struct PacketUnconnectedPong {
    pub time: i64,
    pub guid: u64,
    pub motd: String,
}

#[derive(Clone)]
pub struct ConnectedPong {
    pub client_timestamp: i64,
    pub server_timestamp: i64,
}

#[derive(Clone)]
pub struct OpenConnectionRequest1 {
    pub protocol_version: u8,
    pub mtu_size: u16,
}

#[derive(Clone)]
pub struct OpenConnectionRequest2 {
    pub address: std::net::SocketAddr,
    pub mtu: u16,
    pub guid: u64,
}

#[derive(Clone)]
pub struct OpenConnectionReply1 {
    pub guid: u64,
    pub use_encryption: u8,
    pub mtu_size: u16,
}

#[derive(Clone)]
pub struct OpenConnectionReply2 {
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
    pub server_guid: u64,
}

#[derive(Clone)]
pub struct AlreadyConnected {
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

fn read_magic(cursor: &mut RaknetReader<'_>) -> Result<()> {
    if cursor.read_magic()? {
        Ok(())
    } else {
        Err(RaknetError::PacketHeaderError)
    }
}

pub fn read_packet_ping(buf: &[u8]) -> Result<PacketUnconnectedPing> {
    let mut cursor = RaknetReader::new(buf);
    cursor.read_u8()?;
    let time = cursor.read_i64(Endian::Big)?;
    read_magic(&mut cursor)?;
    Ok(PacketUnconnectedPing {
        time,
        guid: cursor.read_u64(Endian::Big)?,
    })
}

pub fn write_packet_ping(packet: &PacketUnconnectedPing) -> Result<Vec<u8>> {
    let mut cursor = RaknetWriter::new();
    cursor.write_u8(PacketID::UnconnectedPing1.to_u8())?;
    cursor.write_i64(packet.time, Endian::Big)?;
    cursor.write_magic()?;
    cursor.write_u64(packet.guid, Endian::Big)?;
    Ok(cursor.get_raw_payload())
}

pub fn read_packet_pong(buf: &[u8]) -> Result<PacketUnconnectedPong> {
    let mut cursor = RaknetReader::new(buf);
    cursor.read_u8()?;
    let time = cursor.read_i64(Endian::Big)?;
    let guid = cursor.read_u64(Endian::Big)?;
    read_magic(&mut cursor)?;
    Ok(PacketUnconnectedPong {
        time,
        guid,
        motd: cursor.read_string()?,
    })
}

pub fn write_packet_pong(packet: &PacketUnconnectedPong) -> Result<Vec<u8>> {
    let mut cursor = RaknetWriter::new();
    cursor.write_u8(PacketID::UnconnectedPong.to_u8())?;
    cursor.write_i64(packet.time, Endian::Big)?;
    cursor.write_u64(packet.guid, Endian::Big)?;
    cursor.write_magic()?;
    cursor.write_string(&packet.motd)?;
    Ok(cursor.get_raw_payload())
}

pub fn read_packet_connection_open_request_1(buf: &[u8]) -> Result<OpenConnectionRequest1> {
    let mut cursor = RaknetReader::new(buf);
    cursor.read_u8()?;
    read_magic(&mut cursor)?;
    Ok(OpenConnectionRequest1 {
        protocol_version: cursor.read_u8()?,
        // The reported MTU includes the IPv4 and UDP headers.
        mtu_size: buf
            .len()
            .checked_add(28)
            .and_then(|size| u16::try_from(size).ok())
            .ok_or(RaknetError::PacketSizeExceedMTU)?,
    })
}

pub fn write_packet_connection_open_request_1(packet: &OpenConnectionRequest1) -> Result<Vec<u8>> {
    let mut cursor = RaknetWriter::new();
    cursor.write_u8(PacketID::OpenConnectionRequest1.to_u8())?;
    cursor.write_magic()?;
    cursor.write_u8(packet.protocol_version)?;
    // Padding lets the peer discover the largest packet size supported by the path.
    let padding_len = usize::from(packet.mtu_size)
        .checked_sub(46)
        .ok_or(RaknetError::PacketSizeExceedMTU)?;
    cursor.write(&vec![0; padding_len])?;

    Ok(cursor.get_raw_payload())
}

pub fn read_packet_connection_open_request_2(buf: &[u8]) -> Result<OpenConnectionRequest2> {
    let mut cursor = RaknetReader::new(buf);
    cursor.read_u8()?;
    read_magic(&mut cursor)?;
    Ok(OpenConnectionRequest2 {
        address: cursor.read_address()?,
        mtu: cursor.read_u16(Endian::Big)?,
        guid: cursor.read_u64(Endian::Big)?,
    })
}

pub fn write_packet_connection_open_request_2(packet: &OpenConnectionRequest2) -> Result<Vec<u8>> {
    let mut cursor = RaknetWriter::new();
    cursor.write_u8(PacketID::OpenConnectionRequest2.to_u8())?;
    cursor.write_magic()?;
    cursor.write_address(packet.address)?;
    cursor.write_u16(packet.mtu, Endian::Big)?;
    cursor.write_u64(packet.guid, Endian::Big)?;

    Ok(cursor.get_raw_payload())
}

pub fn read_packet_connection_open_reply_1(buf: &[u8]) -> Result<OpenConnectionReply1> {
    let mut cursor = RaknetReader::new(buf);
    cursor.read_u8()?;
    read_magic(&mut cursor)?;
    Ok(OpenConnectionReply1 {
        guid: cursor.read_u64(Endian::Big)?,
        use_encryption: cursor.read_u8()?,
        mtu_size: cursor.read_u16(Endian::Big)?,
    })
}

pub fn write_packet_connection_open_reply_1(packet: &OpenConnectionReply1) -> Result<Vec<u8>> {
    let mut cursor = RaknetWriter::new();
    cursor.write_u8(PacketID::OpenConnectionReply1.to_u8())?;
    cursor.write_magic()?;
    cursor.write_u64(packet.guid, Endian::Big)?;
    cursor.write_u8(packet.use_encryption)?;
    cursor.write_u16(packet.mtu_size, Endian::Big)?;

    Ok(cursor.get_raw_payload())
}

pub fn read_packet_connection_open_reply_2(buf: &[u8]) -> Result<OpenConnectionReply2> {
    let mut cursor = RaknetReader::new(buf);
    cursor.read_u8()?;
    read_magic(&mut cursor)?;
    Ok(OpenConnectionReply2 {
        guid: cursor.read_u64(Endian::Big)?,
        address: cursor.read_address()?,
        mtu: cursor.read_u16(Endian::Big)?,
        encryption_enabled: cursor.read_u8()?,
    })
}

pub fn write_packet_connection_open_reply_2(packet: &OpenConnectionReply2) -> Result<Vec<u8>> {
    let mut cursor = RaknetWriter::new();
    cursor.write_u8(PacketID::OpenConnectionReply2.to_u8())?;
    cursor.write_magic()?;
    cursor.write_u64(packet.guid, Endian::Big)?;
    cursor.write_address(packet.address)?;
    cursor.write_u16(packet.mtu, Endian::Big)?;
    cursor.write_u8(packet.encryption_enabled)?;

    Ok(cursor.get_raw_payload())
}

pub fn _read_packet_already_connected(buf: &[u8]) -> Result<AlreadyConnected> {
    let mut cursor = RaknetReader::new(buf);
    cursor.read_u8()?;
    read_magic(&mut cursor)?;
    Ok(AlreadyConnected {
        guid: cursor.read_u64(Endian::Big)?,
    })
}

pub fn write_packet_already_connected(packet: &AlreadyConnected) -> Result<Vec<u8>> {
    let mut cursor = RaknetWriter::new();
    cursor.write_u8(PacketID::AlreadyConnected.to_u8())?;
    cursor.write_magic()?;
    cursor.write_u64(packet.guid, Endian::Big)?;
    Ok(cursor.get_raw_payload())
}

pub fn read_packet_incompatible_protocol_version(
    buf: &[u8],
) -> Result<IncompatibleProtocolVersion> {
    let mut cursor = RaknetReader::new(buf);
    cursor.read_u8()?;
    read_magic(&mut cursor)?;
    Ok(IncompatibleProtocolVersion {
        server_protocol: cursor.read_u8()?,

        server_guid: cursor.read_u64(Endian::Big)?,
    })
}

pub fn write_packet_incompatible_protocol_version(
    packet: &IncompatibleProtocolVersion,
) -> Result<Vec<u8>> {
    let mut cursor = RaknetWriter::new();
    cursor.write_u8(PacketID::IncompatibleProtocolVersion.to_u8())?;
    cursor.write_u8(packet.server_protocol)?;
    cursor.write_magic()?;
    cursor.write_u64(packet.server_guid, Endian::Big)?;

    Ok(cursor.get_raw_payload())
}

fn read_sequence_records(
    cursor: &mut RaknetReader<'_>,
    record_count: u16,
) -> Result<Vec<(u32, u32)>> {
    // Every record needs at least its tag and one u24. Validate before allocation.
    if usize::from(record_count) > cursor.remaining() / 4 {
        return Err(RaknetError::PacketParseError);
    }
    let mut sequences = Vec::with_capacity(usize::from(record_count));
    for _ in 0..record_count {
        let is_single = cursor.read_u8()?;
        if is_single > 1 {
            return Err(RaknetError::PacketParseError);
        }
        let start = cursor.read_u24(Endian::Little)?;
        let end = if is_single == 1 {
            start
        } else {
            cursor.read_u24(Endian::Little)?
        };
        if end < start {
            return Err(RaknetError::PacketParseError);
        }
        sequences.push((start, end));
    }
    Ok(sequences)
}

fn write_sequence_records(cursor: &mut RaknetWriter, sequences: &[(u32, u32)]) -> Result<()> {
    for &(start, end) in sequences {
        let is_single = u8::from(start == end);
        cursor.write_u8(is_single)?;
        cursor.write_u24(start, Endian::Little)?;
        if is_single == 0 {
            cursor.write_u24(end, Endian::Little)?;
        }
    }
    Ok(())
}

pub fn read_packet_nack(buf: &[u8]) -> Result<Nack> {
    let mut cursor = RaknetReader::new(buf);
    cursor.read_u8()?;
    let record_count = cursor.read_u16(Endian::Big)?;
    let sequences = read_sequence_records(&mut cursor, record_count)?;

    Ok(Nack {
        record_count,
        sequences,
    })
}

pub fn write_packet_nack(packet: &Nack) -> Result<Vec<u8>> {
    let mut cursor = RaknetWriter::new();
    if usize::from(packet.record_count) != packet.sequences.len() {
        return Err(RaknetError::PacketParseError);
    }
    cursor.write_u8(PacketID::Nack.to_u8())?;
    cursor.write_u16(packet.record_count, Endian::Big)?;

    write_sequence_records(&mut cursor, &packet.sequences)?;

    Ok(cursor.get_raw_payload())
}

pub fn read_packet_ack(buf: &[u8]) -> Result<Ack> {
    let mut cursor = RaknetReader::new(buf);
    cursor.read_u8()?;
    let record_count = cursor.read_u16(Endian::Big)?;
    let sequences = read_sequence_records(&mut cursor, record_count)?;
    Ok(Ack {
        record_count,
        sequences,
    })
}

pub fn write_packet_ack(packet: &Ack) -> Result<Vec<u8>> {
    let mut cursor = RaknetWriter::new();
    if usize::from(packet.record_count) != packet.sequences.len() {
        return Err(RaknetError::PacketParseError);
    }
    cursor.write_u8(PacketID::Ack.to_u8())?;
    cursor.write_u16(packet.record_count, Endian::Big)?;

    write_sequence_records(&mut cursor, &packet.sequences)?;

    Ok(cursor.get_raw_payload())
}

pub fn read_packet_connection_request(buf: &[u8]) -> Result<ConnectionRequest> {
    let mut cursor = RaknetReader::new(buf);
    cursor.read_u8()?;
    Ok(ConnectionRequest {
        guid: cursor.read_u64(Endian::Big)?,
        time: cursor.read_i64(Endian::Big)?,
        use_encryption: cursor.read_u8()?,
    })
}

pub fn write_packet_connection_request(packet: &ConnectionRequest) -> Result<Vec<u8>> {
    let mut cursor = RaknetWriter::new();
    cursor.write_u8(PacketID::ConnectionRequest.to_u8())?;
    cursor.write_u64(packet.guid, Endian::Big)?;
    cursor.write_i64(packet.time, Endian::Big)?;
    cursor.write_u8(packet.use_encryption)?;

    Ok(cursor.get_raw_payload())
}

// RakNet peers commonly advertise either ten or twenty internal addresses.
fn read_internal_addresses(cursor: &mut RaknetReader<'_>) -> Result<()> {
    let mut count = 0;
    while cursor.remaining() > 16 && count < 20 {
        cursor.read_address()?;
        count += 1;
    }
    if cursor.remaining() != 16 {
        return Err(RaknetError::PacketParseError);
    }
    Ok(())
}

pub fn read_packet_connection_request_accepted(buf: &[u8]) -> Result<ConnectionRequestAccepted> {
    let mut cursor = RaknetReader::new(buf);
    cursor.read_u8()?;
    let client_address = cursor.read_address()?;
    let system_index = cursor.read_u16(Endian::Big)?;
    read_internal_addresses(&mut cursor)?;
    Ok(ConnectionRequestAccepted {
        client_address,
        system_index,
        request_timestamp: cursor.read_i64(Endian::Big)?,
        accepted_timestamp: cursor.read_i64(Endian::Big)?,
    })
}

pub fn write_packet_connection_request_accepted(
    packet: &ConnectionRequestAccepted,
) -> Result<Vec<u8>> {
    let mut cursor = RaknetWriter::new();
    cursor.write_u8(PacketID::ConnectionRequestAccepted.to_u8())?;
    cursor.write_address(packet.client_address)?;
    cursor.write_u16(packet.system_index, Endian::Big)?;
    let tmp_address = SocketAddr::from(([255, 255, 255, 255], 19132));
    for _ in 0..10 {
        cursor.write_address(tmp_address)?;
    }
    cursor.write_i64(packet.request_timestamp, Endian::Big)?;
    cursor.write_i64(packet.accepted_timestamp, Endian::Big)?;

    Ok(cursor.get_raw_payload())
}

pub fn read_packet_new_incomming_connection(buf: &[u8]) -> Result<NewIncomingConnection> {
    let mut cursor = RaknetReader::new(buf);
    cursor.read_u8()?;
    Ok(NewIncomingConnection {
        server_address: cursor.read_address()?,
        request_timestamp: {
            read_internal_addresses(&mut cursor)?;
            cursor.read_i64(Endian::Big)?
        },
        accepted_timestamp: cursor.read_i64(Endian::Big)?,
    })
}

pub fn write_packet_new_incomming_connection(packet: &NewIncomingConnection) -> Result<Vec<u8>> {
    let mut cursor = RaknetWriter::new();
    cursor.write_u8(PacketID::NewIncomingConnection.to_u8())?;
    cursor.write_address(packet.server_address)?;
    let tmp_address = SocketAddr::from(([0, 0, 0, 0], 0));
    for _ in 0..10 {
        cursor.write_address(tmp_address)?;
    }
    cursor.write_i64(packet.request_timestamp, Endian::Big)?;
    cursor.write_i64(packet.accepted_timestamp, Endian::Big)?;

    Ok(cursor.get_raw_payload())
}

pub fn read_packet_connected_ping(buf: &[u8]) -> Result<ConnectedPing> {
    let mut cursor = RaknetReader::new(buf);
    cursor.read_u8()?;
    Ok(ConnectedPing {
        client_timestamp: cursor.read_i64(Endian::Big)?,
    })
}

pub fn write_packet_connected_ping(packet: &ConnectedPing) -> Result<Vec<u8>> {
    let mut cursor = RaknetWriter::new();
    cursor.write_u8(PacketID::ConnectedPing.to_u8())?;
    cursor.write_i64(packet.client_timestamp, Endian::Big)?;

    Ok(cursor.get_raw_payload())
}

pub fn _read_packet_connected_pong(buf: &[u8]) -> Result<ConnectedPong> {
    let mut cursor = RaknetReader::new(buf);
    cursor.read_u8()?;
    Ok(ConnectedPong {
        client_timestamp: cursor.read_i64(Endian::Big)?,
        server_timestamp: cursor.read_i64(Endian::Big)?,
    })
}

pub fn write_packet_connected_pong(packet: &ConnectedPong) -> Result<Vec<u8>> {
    let mut cursor = RaknetWriter::new();
    cursor.write_u8(PacketID::ConnectedPong.to_u8())?;
    cursor.write_i64(packet.client_timestamp, Endian::Big)?;
    cursor.write_i64(packet.server_timestamp, Endian::Big)?;
    Ok(cursor.get_raw_payload())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ping_rejects_invalid_magic() {
        let packet = PacketUnconnectedPing { time: 1, guid: 2 };
        let mut bytes = write_packet_ping(&packet).unwrap();
        bytes[9] ^= 1;
        assert!(matches!(
            read_packet_ping(&bytes),
            Err(RaknetError::PacketHeaderError)
        ));
    }

    #[test]
    fn ack_writer_rejects_a_mismatched_record_count() {
        let packet = Ack {
            record_count: 1,
            sequences: Vec::new(),
        };
        assert!(matches!(
            write_packet_ack(&packet),
            Err(RaknetError::PacketParseError)
        ));
    }

    #[test]
    fn ack_reader_rejects_reversed_ranges() {
        let bytes = [PacketID::Ack.to_u8(), 0, 1, 0, 10, 0, 0, 2, 0, 0];
        assert!(matches!(
            read_packet_ack(&bytes),
            Err(RaknetError::PacketParseError)
        ));
    }
}

#[test]
fn handshake_timestamps_follow_ten_or_twenty_internal_addresses() {
    for count in [10, 20] {
        let address = "127.0.0.1:19132".parse().unwrap();
        let mut writer = RaknetWriter::new();
        writer
            .write_u8(PacketID::ConnectionRequestAccepted.to_u8())
            .unwrap();
        writer.write_address(address).unwrap();
        writer.write_u16(0, Endian::Big).unwrap();
        for _ in 0..count {
            writer.write_address(address).unwrap();
        }
        writer.write_i64(123, Endian::Big).unwrap();
        writer.write_i64(456, Endian::Big).unwrap();
        let parsed = read_packet_connection_request_accepted(&writer.get_raw_payload()).unwrap();
        assert_eq!(parsed.request_timestamp, 123);
        assert_eq!(parsed.accepted_timestamp, 456);
    }
}

#[test]
fn ack_count_is_validated_before_allocating_records() {
    assert!(read_packet_ack(&[0xc0, 0xff, 0xff]).is_err());
    assert!(read_packet_nack(&[0xa0, 0xff, 0xff]).is_err());
}
