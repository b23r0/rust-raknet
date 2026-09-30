use crate::sequence::{self, ReliableWindow};
use bytes::Bytes;
use std::{
    collections::{HashMap, VecDeque},
    net::SocketAddr,
};

use crate::{datatype::*, error::*, fragment::FragmentQ, raknet_log_debug, utils::*};

/// Delivery and ordering guarantees for a RakNet frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Reliability {
    /// The frame may be lost or arrive out of order.
    Unreliable = 0x00,
    /// Only the newest frame in the ordering stream is delivered.
    UnreliableSequenced = 0x01,
    /// The frame is retransmitted until acknowledged, without ordering guarantees.
    Reliable = 0x02,
    /// The frame is retransmitted and delivered in order within its channel.
    ReliableOrdered = 0x03,
    /// The frame is reliable, but older frames in its ordering stream may be skipped.
    ReliableSequenced = 0x04,
}

impl Reliability {
    pub fn to_u8(&self) -> u8 {
        *self as u8
    }

    pub fn from(flags: u8) -> Result<Self> {
        Self::try_from(flags)
    }
}

impl TryFrom<u8> for Reliability {
    type Error = RaknetError;

    fn try_from(flags: u8) -> Result<Self> {
        match flags {
            0x00 => Ok(Self::Unreliable),
            0x01 => Ok(Self::UnreliableSequenced),
            0x02 => Ok(Self::Reliable),
            0x03 => Ok(Self::ReliableOrdered),
            0x04 => Ok(Self::ReliableSequenced),
            _ => Err(RaknetError::IncorrectReliability),
        }
    }
}

const NEEDS_B_AND_AS_FLAG: u8 = 0x4;
const CONTINUOUS_SEND_FLAG: u8 = 0x8;

#[derive(Clone)]
pub struct FrameSetPacket {
    pub sequence_number: u32,
    pub flags: u8,
    pub length_in_bytes: u16,
    pub reliable_frame_index: u32,
    pub sequenced_frame_index: u32,
    pub ordered_frame_index: u32,
    pub order_channel: u8,
    pub compound_size: u32,
    pub compound_id: u16,
    pub fragment_index: u32,
    pub data: Bytes,
}

impl FrameSetPacket {
    pub fn new(r: Reliability, data: Vec<u8>) -> Self {
        let flag = r.to_u8() << 5;

        Self {
            sequence_number: 0,
            flags: flag,
            length_in_bytes: data.len() as u16,
            reliable_frame_index: 0,
            sequenced_frame_index: 0,
            ordered_frame_index: 0,
            order_channel: 0,
            compound_size: 0,
            compound_id: 0,
            fragment_index: 0,
            data: data.into(),
        }
    }

    #[cfg(test)]
    pub fn deserialize(buf: &[u8]) -> Result<(Self, bool)> {
        let mut reader = RaknetReader::new(buf);
        let input_len = buf.len() as u64;

        let mut ret = Self {
            sequence_number: 0,
            flags: 0,
            length_in_bytes: 0,
            reliable_frame_index: 0,
            sequenced_frame_index: 0,
            ordered_frame_index: 0,
            order_channel: 0,
            compound_size: 0,
            compound_id: 0,
            fragment_index: 0,
            data: Bytes::new(),
        };

        let packet_id = reader.read_u8()?;
        if !(0x80..=0x8d).contains(&packet_id) {
            return Err(RaknetError::PacketHeaderError);
        }
        ret.sequence_number = reader.read_u24(Endian::Little)?;
        ret.flags = reader.read_u8()?;

        let length_in_bits = reader.read_u16(Endian::Big)?;
        if length_in_bits % 8 != 0 {
            return Err(RaknetError::PacketParseError);
        }
        ret.length_in_bytes = length_in_bits / 8;

        if ret.is_reliable()? {
            ret.reliable_frame_index = reader.read_u24(Endian::Little)?;
        }

        if ret.is_sequenced()? {
            ret.sequenced_frame_index = reader.read_u24(Endian::Little)?;
        }
        if ret.is_ordered()? {
            ret.ordered_frame_index = reader.read_u24(Endian::Little)?;
            ret.order_channel = reader.read_u8()?;
        }

        if ret.is_fragment() {
            ret.compound_size = reader.read_u32(Endian::Big)?;
            ret.compound_id = reader.read_u16(Endian::Big)?;
            ret.fragment_index = reader.read_u32(Endian::Big)?;
        }

        let mut data = vec![0; usize::from(ret.length_in_bytes)];
        reader.read(&mut data)?;
        ret.data = data.into();

        if ret.is_fragment() && (ret.compound_size == 0 || ret.fragment_index >= ret.compound_size)
        {
            return Err(RaknetError::PacketParseError);
        }

        Ok((ret, reader.pos() == input_len))
    }

    pub fn serialize(&self) -> Result<Vec<u8>> {
        if self.data.len() != usize::from(self.length_in_bytes)
            || self.data.len() > usize::from(u16::MAX / 8)
            || (self.is_fragment()
                && (self.compound_size == 0 || self.fragment_index >= self.compound_size))
        {
            return Err(RaknetError::PacketParseError);
        }

        let mut writer = RaknetWriter::with_capacity(self._size()?);
        let mut id = 0x80 | NEEDS_B_AND_AS_FLAG;

        // Set the continuation bit for all fragments after the first.
        if (self.flags & 16) != 0 && self.fragment_index != 0 {
            id |= CONTINUOUS_SEND_FLAG;
        }

        writer.write_u8(id)?;
        writer.write_u24(self.sequence_number, Endian::Little)?;

        // The top three bits encode the reliability mode.
        writer.write_u8(self.flags)?;
        writer.write_u16(self.length_in_bytes * 8, Endian::Big)?;

        if self.is_reliable()? {
            writer.write_u24(self.reliable_frame_index, Endian::Little)?;
        }

        if self.is_sequenced()? {
            writer.write_u24(self.sequenced_frame_index, Endian::Little)?;
        }
        if self.is_ordered()? {
            writer.write_u24(self.ordered_frame_index, Endian::Little)?;
            writer.write_u8(self.order_channel)?;
        }

        // Bit 4 marks a fragmented frame.
        if (self.flags & 16) != 0 {
            writer.write_u32(self.compound_size, Endian::Big)?;
            writer.write_u16(self.compound_id, Endian::Big)?;
            writer.write_u32(self.fragment_index, Endian::Big)?;
        }
        writer.write(self.data.as_ref())?;

        Ok(writer.get_raw_payload())
    }

    pub fn is_fragment(&self) -> bool {
        (self.flags & 16) != 0
    }

    pub fn is_reliable(&self) -> Result<bool> {
        let r = Reliability::from((self.flags & 224) >> 5)?;
        Ok(matches!(
            r,
            Reliability::Reliable | Reliability::ReliableOrdered | Reliability::ReliableSequenced
        ))
    }

    pub fn is_ordered(&self) -> Result<bool> {
        let r = Reliability::from((self.flags & 224) >> 5)?;
        Ok(matches!(
            r,
            Reliability::UnreliableSequenced
                | Reliability::ReliableOrdered
                | Reliability::ReliableSequenced
        ))
    }

    pub fn is_sequenced(&self) -> Result<bool> {
        let r = Reliability::from((self.flags & 224) >> 5)?;
        Ok(matches!(
            r,
            Reliability::UnreliableSequenced | Reliability::ReliableSequenced
        ))
    }
    pub fn reliability(&self) -> Result<Reliability> {
        Reliability::from((self.flags & 224) >> 5)
    }

    pub fn _size(&self) -> Result<usize> {
        let mut ret = 0;
        // id
        ret += 1;
        // sequence number
        ret += 3;
        // flags
        ret += 1;
        // length_in_bits
        ret += 2;

        if self.is_reliable()? {
            // reliable frame index
            ret += 3;
        }
        if self.is_sequenced()? {
            // sequenced frame index
            ret += 3;
        }
        if self.is_ordered()? {
            // Ordered frame index and channel.
            ret += 4;
        }
        if (self.flags & 16) != 0 {
            // Compound size, compound ID, and fragment index.
            ret += 10;
        }
        //body
        ret += self.data.len();
        Ok(ret)
    }
}

pub struct FrameVec {
    pub frames: Vec<FrameSetPacket>,
}

impl FrameVec {
    pub fn new(buf: &[u8]) -> Result<Self> {
        let mut reader = RaknetReader::new(buf);
        let id = reader.read_u8()?;
        if !(0x80..=0x8d).contains(&id) {
            return Err(RaknetError::PacketHeaderError);
        }
        let sequence_number = reader.read_u24(Endian::Little)?;
        if reader.pos() == buf.len() as u64 {
            return Err(RaknetError::PacketParseError);
        }
        let mut frames = Vec::new();

        while reader.pos() < buf.len() as u64 {
            let mut frame = FrameSetPacket {
                sequence_number,
                flags: 0,
                length_in_bytes: 0,
                reliable_frame_index: 0,
                sequenced_frame_index: 0,
                ordered_frame_index: 0,
                order_channel: 0,
                compound_size: 0,
                compound_id: 0,
                fragment_index: 0,
                data: Bytes::new(),
            };

            frame.flags = reader.read_u8()?;
            let length_in_bits = reader.read_u16(Endian::Big)?;
            if length_in_bits % 8 != 0 {
                return Err(RaknetError::PacketParseError);
            }
            frame.length_in_bytes = length_in_bits / 8;

            if frame.is_reliable()? {
                frame.reliable_frame_index = reader.read_u24(Endian::Little)?;
            }
            if frame.is_sequenced()? {
                frame.sequenced_frame_index = reader.read_u24(Endian::Little)?;
            }
            if frame.is_ordered()? {
                frame.ordered_frame_index = reader.read_u24(Endian::Little)?;
                frame.order_channel = reader.read_u8()?;
            }
            if frame.is_fragment() {
                frame.compound_size = reader.read_u32(Endian::Big)?;
                frame.compound_id = reader.read_u16(Endian::Big)?;
                frame.fragment_index = reader.read_u32(Endian::Big)?;
                if frame.compound_size == 0 || frame.fragment_index >= frame.compound_size {
                    return Err(RaknetError::PacketParseError);
                }
            }

            let data = reader.read_slice(usize::from(frame.length_in_bytes))?;
            frame.data = Bytes::copy_from_slice(data);
            frames.push(frame);
        }

        Ok(Self { frames })
    }
}

#[derive(Default)]
pub struct ACKSet {
    ack: Vec<(u32, u32)>,
    nack: Vec<(u32, u32)>,
    next_expected: u32,
}

impl ACKSet {
    pub fn insert(&mut self, s: u32) {
        // Remove a hole that arrived before the next NACK was emitted.
        let mut split = None;
        self.nack.retain_mut(|(start, end)| {
            if s < *start || s > *end {
                return true;
            }
            if *start == *end {
                return false;
            }
            if s == *start {
                *start += 1;
            } else if s == *end {
                *end -= 1;
            } else {
                split = Some((s + 1, *end));
                *end = s - 1;
            }
            true
        });
        if let Some(range) = split {
            self.nack.push(range);
        }
        if s == self.next_expected || sequence::newer(s, self.next_expected) {
            if s != self.next_expected {
                if s > self.next_expected {
                    self.nack.push((self.next_expected, s - 1));
                } else {
                    self.nack.push((self.next_expected, sequence::MASK));
                    if s != 0 {
                        self.nack.push((0, s - 1));
                    }
                }
            }
            self.next_expected = sequence::next(s);
        }

        for i in 0..self.ack.len() {
            let a = self.ack[i];
            if (a.0..=a.1).contains(&s) {
                return;
            }
            if a.0 != 0 && s == a.0 - 1 {
                self.ack[i].0 = s;
                return;
            }
            if s == a.1.saturating_add(1) {
                self.ack[i].1 = s;
                return;
            }
        }
        self.ack.push((s, s));
    }

    pub fn get_ack(&mut self) -> Vec<(u32, u32)> {
        std::mem::take(&mut self.ack)
    }

    pub fn get_nack(&mut self) -> Vec<(u32, u32)> {
        std::mem::take(&mut self.nack)
    }
}

/// Channel zero is the common path; other channels allocate only when used.
#[derive(Default)]
struct ChannelIndexes {
    primary: u32,
    others: HashMap<u8, u32>,
}

impl ChannelIndexes {
    fn get(&self, channel: u8) -> u32 {
        if channel == 0 {
            self.primary
        } else {
            self.others.get(&channel).copied().unwrap_or(0)
        }
    }

    fn get_mut(&mut self, channel: u8) -> &mut u32 {
        if channel == 0 {
            &mut self.primary
        } else {
            self.others.entry(channel).or_default()
        }
    }

    #[cfg(test)]
    fn insert(&mut self, channel: u8, index: u32) {
        *self.get_mut(channel) = index;
    }
}

#[derive(Default)]
pub struct RecvQ {
    sequenced_frame_indexes: ChannelIndexes,
    last_ordered_indexes: ChannelIndexes,
    sequence_number_ackset: ACKSet,
    packets: HashMap<u32, FrameSetPacket>,
    ordered_packets: HashMap<(u8, u32), FrameSetPacket>,
    fragment_queue: FragmentQ,
    reliable_window: ReliableWindow,
    ordered_bytes: usize,
    ready_ordered: Vec<FrameSetPacket>,
    ready_bytes: usize,
}

impl RecvQ {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, frame: FrameSetPacket) -> Result<()> {
        let reliability = frame.reliability()?;
        self.sequence_number_ackset.insert(frame.sequence_number);
        if frame.is_reliable()? && !self.reliable_window.accept(frame.reliable_frame_index)? {
            return Ok(());
        }

        match reliability {
            Reliability::Unreliable => {
                self.packets.entry(frame.sequence_number).or_insert(frame);
            }
            Reliability::UnreliableSequenced => {
                let channel = frame.order_channel;
                let sequence_index = frame.sequenced_frame_index;
                let sequence_number = frame.sequence_number;
                let last_index = self.sequenced_frame_indexes.get_mut(channel);
                if sequence_index == *last_index || sequence::newer(sequence_index, *last_index) {
                    self.packets.entry(sequence_number).or_insert(frame);
                    *last_index = sequence::next(sequence_index);
                }
            }
            Reliability::Reliable => {
                self.packets.insert(frame.sequence_number, frame);
            }
            Reliability::ReliableOrdered => {
                let expected_index = self.last_ordered_indexes.get(frame.order_channel);
                if sequence::newer(expected_index, frame.ordered_frame_index) {
                    return Ok(());
                }
                if sequence::distance(frame.ordered_frame_index, expected_index)
                    >= sequence::RECEIVE_WINDOW
                {
                    return Err(RaknetError::PacketParseError);
                }
                let complete = if frame.is_fragment() {
                    self.fragment_queue.insert_and_take_completed(frame)?
                } else {
                    Some(frame)
                };
                if let Some(frame) = complete {
                    let key = (frame.order_channel, frame.ordered_frame_index);
                    if self.ordered_packets.contains_key(&key) {
                        return Ok(());
                    }
                    if self.ordered_packets.len() + self.ready_ordered.len()
                        >= sequence::RECEIVE_WINDOW as usize
                        || self.ordered_bytes + self.ready_bytes + frame.data.len()
                            > 64 * 1024 * 1024
                    {
                        return Err(RaknetError::PacketParseError);
                    }
                    if frame.ordered_frame_index == expected_index {
                        let channel = frame.order_channel;
                        self.ready_bytes += frame.data.len();
                        self.ready_ordered.push(frame);
                        let expected = self.last_ordered_indexes.get_mut(channel);
                        *expected = sequence::next(*expected);
                        // Contiguous traffic bypasses the reordering hash table.
                        while !self.ordered_packets.is_empty() {
                            let Some(frame) = self.ordered_packets.remove(&(channel, *expected))
                            else {
                                break;
                            };
                            self.ordered_bytes -= frame.data.len();
                            self.ready_bytes += frame.data.len();
                            self.ready_ordered.push(frame);
                            *expected = sequence::next(*expected);
                        }
                    } else {
                        self.ordered_bytes += frame.data.len();
                        self.ordered_packets.insert(key, frame);
                    }
                }
            }

            Reliability::ReliableSequenced => {
                let channel = frame.order_channel;
                let sequence_index = frame.sequenced_frame_index;
                let sequence_number = frame.sequence_number;
                let last_index = self.sequenced_frame_indexes.get_mut(channel);
                if sequence_index == *last_index || sequence::newer(sequence_index, *last_index) {
                    self.packets.entry(sequence_number).or_insert(frame);
                    *last_index = sequence::next(sequence_index);
                }
            }
        }
        Ok(())
    }

    pub fn get_ack(&mut self) -> Vec<(u32, u32)> {
        self.sequence_number_ackset.get_ack()
    }

    pub fn get_nack(&mut self) -> Vec<(u32, u32)> {
        self.sequence_number_ackset.get_nack()
    }

    pub fn flush(&mut self, _peer_addr: &SocketAddr) -> Vec<FrameSetPacket> {
        let mut ret = std::mem::take(&mut self.ready_ordered);
        self.ready_bytes = 0;
        if self.packets.is_empty() {
            return ret;
        }

        let mut packets: Vec<_> = self.packets.drain().map(|(_, frame)| frame).collect();
        packets.sort_unstable_by_key(|frame| frame.sequence_number);
        ret.extend(packets);
        ret
    }
    pub fn fragments_expired(&self) -> bool {
        self.fragment_queue.expired()
    }

    pub fn get_ordered_packet(&self) -> usize {
        self.ordered_packets.len() + self.ready_ordered.len()
    }

    pub fn get_fragment_queue_size(&self) -> usize {
        self.fragment_queue.size()
    }

    pub fn get_ordered_keys(&self) -> Vec<(u8, u32)> {
        self.ordered_packets.keys().copied().collect()
    }

    pub fn get_size(&self) -> usize {
        self.packets.len()
    }
}

pub struct SendQ {
    mtu: u16,
    ack_sequence_number: u32,
    sequence_number: u32,
    reliable_frame_index: u32,
    sequenced_frame_indexes: ChannelIndexes,
    ordered_frame_indexes: ChannelIndexes,
    compound_id: u16,
    // Each entry stores a frame, send state, last send time, retry count, and prior sequence IDs.
    packets: VecDeque<FrameSetPacket>,
    queued_unreliable: usize,
    buffered_bytes: usize,
    rto: i64,
    srtt: i64,
    sent_packet: Vec<(FrameSetPacket, bool, i64, u32, Vec<u32>)>,
}

impl SendQ {
    pub const DEFAULT_TIMEOUT_MILLS: i64 = 50;
    const MAX_IN_FLIGHT_PACKETS: usize = 64;
    const MAX_BUFFERED_BYTES: usize = 64 * 1024 * 1024;
    const SEND_HIGH_WATER: usize = 256 * 1024;
    const FRAME_BUDGET: usize = 128;

    const RTO_UBOUND: i64 = 12000;
    const RTO_LBOUND: i64 = 50;

    pub fn new(mtu: u16) -> Self {
        Self {
            mtu,
            ack_sequence_number: sequence::MASK,
            sequence_number: 0,
            packets: VecDeque::new(),
            queued_unreliable: 0,
            buffered_bytes: 0,
            sent_packet: vec![],
            reliable_frame_index: 0,
            sequenced_frame_indexes: ChannelIndexes::default(),
            ordered_frame_indexes: ChannelIndexes::default(),
            compound_id: 0,

            rto: SendQ::DEFAULT_TIMEOUT_MILLS,
            srtt: SendQ::DEFAULT_TIMEOUT_MILLS,
        }
    }

    fn required_bytes(&self, reliability: Reliability, len: usize) -> Result<usize> {
        let payload = usize::from(self.mtu)
            .checked_sub(60)
            .filter(|n| *n > 0)
            .ok_or(RaknetError::PacketSizeExceedMTU)?;
        let frames = if reliability == Reliability::ReliableOrdered && len > payload {
            let frames = len.div_ceil(payload);
            if frames > crate::fragment::MAX_FRAGMENTS {
                return Err(RaknetError::PacketSizeExceedMTU);
            }
            frames
        } else {
            1
        };
        let bytes = frames
            .checked_mul(Self::FRAME_BUDGET)
            .and_then(|n| n.checked_add(len))
            .filter(|n| *n <= Self::MAX_BUFFERED_BYTES)
            .ok_or(RaknetError::PacketSizeExceedMTU)?;
        Ok(bytes)
    }

    pub fn has_capacity(&self, reliability: Reliability, len: usize) -> Result<bool> {
        let required = self.required_bytes(reliability, len)?;
        // Admit a single large message while applying backpressure to bursts.
        Ok(self.buffered_bytes.saturating_add(required) <= Self::SEND_HIGH_WATER.max(required))
    }

    pub fn insert(&mut self, reliability: Reliability, buf: &[u8]) -> Result<()> {
        self.insert_with_order_channel(reliability, buf, 0)
    }

    pub fn insert_with_order_channel(
        &mut self,
        reliability: Reliability,
        buf: &[u8],
        order_channel: u8,
    ) -> Result<()> {
        let reserved = self.required_bytes(reliability, buf.len())?;
        if reserved > Self::MAX_BUFFERED_BYTES - self.buffered_bytes {
            return Err(RaknetError::PacketSizeExceedMTU);
        }
        let max_payload = self
            .mtu
            .checked_sub(60)
            .map(usize::from)
            .filter(|size| *size > 0)
            .ok_or(RaknetError::PacketSizeExceedMTU)?;

        match reliability {
            Reliability::Unreliable => {
                if buf.len() > max_payload {
                    return Err(RaknetError::PacketSizeExceedMTU);
                }

                let frame = FrameSetPacket::new(reliability, buf.to_vec());
                self.packets.push_back(frame);
            }
            Reliability::UnreliableSequenced => {
                if buf.len() > max_payload {
                    return Err(RaknetError::PacketSizeExceedMTU);
                }

                let sequenced_frame_index = {
                    let index = self.sequenced_frame_indexes.get_mut(order_channel);
                    let current = *index;
                    *index = sequence::next(*index);
                    current
                };
                let ordered_frame_index = self.ordered_frame_indexes.get(order_channel);
                let mut frame = FrameSetPacket::new(reliability, buf.to_vec());
                frame.order_channel = order_channel;
                frame.ordered_frame_index = ordered_frame_index;
                frame.sequenced_frame_index = sequenced_frame_index;
                self.packets.push_back(frame);
            }
            Reliability::Reliable => {
                if buf.len() > max_payload {
                    return Err(RaknetError::PacketSizeExceedMTU);
                }

                let mut frame = FrameSetPacket::new(reliability, buf.to_vec());
                frame.reliable_frame_index = self.reliable_frame_index;
                self.packets.push_back(frame);
                self.reliable_frame_index = sequence::next(self.reliable_frame_index);
            }
            Reliability::ReliableOrdered => {
                if buf.len() <= max_payload {
                    let mut frame = FrameSetPacket::new(reliability, buf.to_vec());
                    frame.order_channel = order_channel;
                    frame.reliable_frame_index = self.reliable_frame_index;
                    frame.ordered_frame_index = *self.ordered_frame_indexes.get_mut(order_channel);
                    self.packets.push_back(frame);
                    self.reliable_frame_index = sequence::next(self.reliable_frame_index);
                } else {
                    let compound_size = buf.len().div_ceil(max_payload);
                    let compound_size = u32::try_from(compound_size)
                        .map_err(|_| RaknetError::PacketSizeExceedMTU)?;
                    let ordered_frame_index = *self.ordered_frame_indexes.get_mut(order_channel);

                    for (fragment_index, chunk) in buf.chunks(max_payload).enumerate() {
                        let mut frame = FrameSetPacket::new(reliability, chunk.to_vec());
                        frame.flags |= 16;
                        frame.compound_size = compound_size;
                        frame.compound_id = self.compound_id;
                        frame.fragment_index = fragment_index as u32;
                        frame.order_channel = order_channel;
                        frame.reliable_frame_index = self.reliable_frame_index;
                        frame.ordered_frame_index = ordered_frame_index;
                        self.packets.push_back(frame);
                        self.reliable_frame_index = sequence::next(self.reliable_frame_index);
                    }
                    self.compound_id = self.compound_id.wrapping_add(1);
                }
                let index = self.ordered_frame_indexes.get_mut(order_channel);
                *index = sequence::next(*index);
            }
            Reliability::ReliableSequenced => {
                if buf.len() > max_payload {
                    return Err(RaknetError::PacketSizeExceedMTU);
                }

                let sequenced_frame_index = {
                    let index = self.sequenced_frame_indexes.get_mut(order_channel);
                    let current = *index;
                    *index = sequence::next(*index);
                    current
                };
                let ordered_frame_index = self.ordered_frame_indexes.get(order_channel);
                let mut frame = FrameSetPacket::new(reliability, buf.to_vec());
                frame.order_channel = order_channel;
                frame.reliable_frame_index = self.reliable_frame_index;
                frame.sequenced_frame_index = sequenced_frame_index;
                frame.ordered_frame_index = ordered_frame_index;
                self.packets.push_back(frame);
                self.reliable_frame_index = sequence::next(self.reliable_frame_index);
            }
        };
        self.buffered_bytes += reserved;
        if matches!(
            reliability,
            Reliability::Unreliable | Reliability::UnreliableSequenced
        ) {
            self.queued_unreliable += 1;
        }
        Ok(())
    }

    fn update_rto(&mut self, rtt: i64) {
        self.srtt = (self.srtt * 4 + rtt) / 5;
        self.rto = (self.srtt * 3 / 2).clamp(Self::RTO_LBOUND, Self::RTO_UBOUND);
    }

    pub fn get_rto(&self) -> i64 {
        self.rto
    }

    pub fn nack(&mut self, sequence: u32, tick: i64) {
        for i in 0..self.sent_packet.len() {
            let item = &mut self.sent_packet[i];
            if item.1 && item.0.sequence_number == sequence {
                raknet_log_debug!(
                    "packet {}-{}-{} nack {} times",
                    item.0.sequence_number,
                    item.0.reliable_frame_index,
                    item.0.ordered_frame_index,
                    item.3 + 1
                );
                let previous = item.0.sequence_number;
                item.0.sequence_number = self.sequence_number;
                self.sequence_number = sequence::next(self.sequence_number);
                item.1 = false;
                item.2 = tick;

                if item.4.len() == 64 {
                    item.4.remove(0);
                }
                item.4.push(previous);
            }
        }
    }

    pub fn ack(&mut self, sequence: u32, tick: i64) {
        // Ignore ACKs outside the actual send history before advancing any state.
        let Some(index) = self
            .sent_packet
            .iter()
            .position(|item| item.0.sequence_number == sequence || item.4.contains(&sequence))
        else {
            return;
        };
        let item = self.sent_packet.remove(index);
        self.buffered_bytes -= item.0.data.len() + Self::FRAME_BUDGET;
        if sequence::newer(sequence, self.ack_sequence_number) {
            let span = sequence::distance(sequence, self.ack_sequence_number);
            let missing: Vec<_> = self
                .sent_packet
                .iter()
                .filter_map(|item| {
                    let distance =
                        sequence::distance(item.0.sequence_number, self.ack_sequence_number);
                    (distance > 0 && distance < span).then_some(item.0.sequence_number)
                })
                .collect();
            for id in missing {
                self.nack(id, tick);
            }
            self.ack_sequence_number = sequence;
        }
        // A retransmitted frame has an ambiguous RTT sample (Karn's algorithm).
        if item.3 == 0 {
            self.update_rto(tick.saturating_sub(item.2).max(0));
        }
    }

    pub fn ack_ranges(&mut self, ranges: &[(u32, u32)], tick: i64) {
        if let [(start, end)] = ranges {
            if start == end {
                self.ack(*start, tick);
                return;
            }
        }
        let acknowledged: Vec<_> = self
            .sent_packet
            .iter()
            .filter_map(|item| {
                std::iter::once(item.0.sequence_number)
                    .chain(item.4.iter().copied())
                    .find(|id| {
                        ranges
                            .iter()
                            .any(|&(start, end)| start <= *id && *id <= end)
                    })
            })
            .collect();
        for id in acknowledged {
            self.ack(id, tick);
        }
    }

    pub fn nack_ranges(&mut self, ranges: &[(u32, u32)], tick: i64) {
        if let [(start, end)] = ranges {
            if start == end {
                self.nack(*start, tick);
                return;
            }
        }
        let missing: Vec<_> = self
            .sent_packet
            .iter()
            .filter_map(|item| {
                let id = item.0.sequence_number;
                ranges
                    .iter()
                    .any(|&(start, end)| start <= id && id <= end)
                    .then_some(id)
            })
            .collect();
        for id in missing {
            self.nack(id, tick);
        }
    }

    fn tick(&mut self, tick: i64) {
        for i in 0..self.sent_packet.len() {
            let p = &mut self.sent_packet[i];

            let mut cur_rto = self.rto;

            // Bound the existing 1.5x retry backoff, including long-lived losses.
            for _ in 0..p.3.min(32) {
                cur_rto = ((cur_rto as f64 * 1.5) as i64).min(Self::RTO_UBOUND);
            }

            if p.1 && tick - p.2 >= cur_rto {
                let previous = p.0.sequence_number;
                p.0.sequence_number = self.sequence_number;
                self.sequence_number = sequence::next(self.sequence_number);
                p.1 = false;
                if p.4.len() == 64 {
                    p.4.remove(0);
                }
                p.4.push(previous);
            }
        }
    }

    pub fn flush(&mut self, tick: i64, peer_addr: &SocketAddr) -> Vec<FrameSetPacket> {
        self.tick(tick);

        let mut ret = Vec::new();
        self.sent_packet
            .sort_unstable_by_key(|packet| packet.0.sequence_number);

        for packet in &mut self.sent_packet {
            if !packet.1 {
                raknet_log_debug!(
                    "{} , packet {}-{}-{} resend {} times",
                    peer_addr,
                    packet.0.sequence_number,
                    packet.0.reliable_frame_index,
                    packet.0.ordered_frame_index,
                    packet.3 + 1
                );
                ret.push(packet.0.clone());
                packet.1 = true;
                packet.2 = tick;
                packet.3 = packet.3.saturating_add(1);
            }
        }

        let queued_count = self.packets.len();
        for _ in 0..queued_count {
            if self.sent_packet.len() >= Self::MAX_IN_FLIGHT_PACKETS && self.queued_unreliable == 0
            {
                break;
            }
            let Some(mut packet) = self.packets.pop_front() else {
                break;
            };
            let reliable = packet.is_reliable().unwrap_or(false);
            if reliable && self.sent_packet.len() >= Self::MAX_IN_FLIGHT_PACKETS {
                self.packets.push_back(packet);
                continue;
            }

            packet.sequence_number = self.sequence_number;
            self.sequence_number = sequence::next(self.sequence_number);

            if reliable {
                ret.push(packet.clone());
                self.sent_packet.push((packet, true, tick, 0, Vec::new()));
            } else {
                self.queued_unreliable -= 1;
                self.buffered_bytes -= packet.data.len() + Self::FRAME_BUDGET;
                ret.push(packet);
            }
        }

        ret
    }

    pub fn is_empty(&self) -> bool {
        self.packets.is_empty() && self.sent_packet.is_empty()
    }

    pub fn get_reliable_queue_size(&self) -> usize {
        self.packets.len()
    }

    pub fn get_sent_queue_size(&self) -> usize {
        self.sent_packet.len()
    }
}

#[test]
fn malformed_datagrams_are_rejected() {
    assert!(FrameVec::new(&[0x80, 0, 0]).is_err());
    assert!(FrameVec::new(&[0x10, 0, 0, 0]).is_err());
}

#[test]
fn send_queue_rejects_an_invalid_mtu() {
    let mut send_queue = SendQ::new(59);
    assert!(send_queue.insert(Reliability::Reliable, &[0xfe]).is_err());
}

#[tokio::test]
async fn test_recvq_orders_channels_independently() {
    let mut recvq = RecvQ::new();

    let mut later_on_channel_one = FrameSetPacket::new(Reliability::ReliableOrdered, vec![1]);
    later_on_channel_one.sequence_number = 0;
    later_on_channel_one.order_channel = 1;
    later_on_channel_one.ordered_frame_index = 1;
    recvq.insert(later_on_channel_one).unwrap();

    let mut first_on_channel_two = FrameSetPacket::new(Reliability::ReliableOrdered, vec![2]);
    first_on_channel_two.sequence_number = 1;
    first_on_channel_two.reliable_frame_index = 1;
    first_on_channel_two.order_channel = 2;
    first_on_channel_two.ordered_frame_index = 0;
    recvq.insert(first_on_channel_two).unwrap();

    let ready = recvq.flush(&"127.0.0.1:0".parse().unwrap());
    assert_eq!(ready.len(), 1);
    assert_eq!(ready[0].order_channel, 2);

    let mut first_on_channel_one = FrameSetPacket::new(Reliability::ReliableOrdered, vec![0]);
    first_on_channel_one.sequence_number = 2;
    first_on_channel_one.reliable_frame_index = 2;
    first_on_channel_one.order_channel = 1;
    first_on_channel_one.ordered_frame_index = 0;
    recvq.insert(first_on_channel_one).unwrap();

    let ready = recvq.flush(&"127.0.0.1:0".parse().unwrap());
    assert_eq!(ready.len(), 2);
    assert_eq!(ready[0].ordered_frame_index, 0);
    assert_eq!(ready[1].ordered_frame_index, 1);
    assert!(ready.iter().all(|frame| frame.order_channel == 1));
}

#[tokio::test]
async fn test_sendq_maintains_ordered_indexes_per_channel() {
    let mut sendq = SendQ::new(1500);
    sendq
        .insert_with_order_channel(Reliability::ReliableOrdered, &[0xfe, 1], 3)
        .unwrap();
    sendq
        .insert_with_order_channel(Reliability::ReliableOrdered, &[0xfe, 2], 9)
        .unwrap();

    let sent = sendq.flush(0, &"127.0.0.1:0".parse().unwrap());
    assert_eq!(sent.len(), 2);
    assert_eq!(sent[0].order_channel, 3);
    assert_eq!(sent[0].ordered_frame_index, 0);
    assert_eq!(sent[1].order_channel, 9);
    assert_eq!(sent[1].ordered_frame_index, 0);
}

#[tokio::test]
async fn test_ackset() {
    let mut ackset = ACKSet::default();

    ackset.insert(0);
    ackset.insert(1);
    ackset.insert(2);
    ackset.insert(4);

    let acks = ackset.get_ack();

    assert!(acks == vec![(0, 2), (4, 4)]);

    let mut ackset = ACKSet::default();

    ackset.insert(0);
    ackset.insert(1);
    ackset.insert(2);
    ackset.insert(6);

    let acks = ackset.get_ack();

    assert!(acks == vec![(0, 2), (6, 6)]);

    let acks = ackset.get_ack();

    assert!(acks == vec![]);

    ackset.insert(0);
    ackset.insert(2);

    let acks = ackset.get_ack();

    assert!(acks == vec![(0, 0), (2, 2)]);
}

#[tokio::test]
async fn test_frame_serialize_deserialize() {
    // Captured first frame datagram from a Bedrock 1.18.12 client.
    let p: Vec<u8> = [
        132, 0, 0, 0, 64, 0, 144, 0, 0, 0, 9, 146, 33, 7, 47, 57, 18, 128, 111, 0, 0, 0, 0, 20,
        200, 47, 41, 0,
    ]
    .to_vec();

    let a = FrameSetPacket::deserialize(&p).unwrap();
    assert!(a.0.serialize().unwrap() == p);
}

#[tokio::test]
async fn test_recvq() {
    let mut r = RecvQ::new();
    let mut p = FrameSetPacket::new(Reliability::Reliable, vec![]);
    p.sequence_number = 0;
    p.ordered_frame_index = 0;
    r.insert(p).unwrap();

    let mut p = FrameSetPacket::new(Reliability::Reliable, vec![]);
    p.sequence_number = 1;
    p.reliable_frame_index = 1;
    p.ordered_frame_index = 1;
    r.insert(p).unwrap();

    let ret = r.flush(&"0.0.0.0:0".parse().unwrap());
    assert!(ret.len() == 2);
}

#[tokio::test]
async fn test_recvq_fragment() {
    let mut r = RecvQ::new();
    let mut p = FrameSetPacket::new(Reliability::ReliableOrdered, vec![1]);
    p.flags |= 16;
    p.sequence_number = 0;
    p.ordered_frame_index = 0;
    p.compound_id = 1;
    p.compound_size = 3;
    p.fragment_index = 0;
    p.order_channel = 7;
    r.insert(p).unwrap();

    let mut p = FrameSetPacket::new(Reliability::ReliableOrdered, vec![2]);
    p.flags |= 16;
    p.sequence_number = 1;
    p.reliable_frame_index = 1;
    p.ordered_frame_index = 0;
    p.compound_id = 1;
    p.compound_size = 3;
    p.fragment_index = 1;
    p.order_channel = 7;
    r.insert(p).unwrap();

    let mut p = FrameSetPacket::new(Reliability::ReliableOrdered, vec![3]);
    p.flags |= 16;
    p.sequence_number = 2;
    p.reliable_frame_index = 2;
    p.ordered_frame_index = 0;
    p.compound_id = 1;
    p.compound_size = 3;
    p.fragment_index = 2;
    p.order_channel = 7;
    r.insert(p).unwrap();

    let ret = r.flush(&"0.0.0.0:0".parse().unwrap());
    assert!(ret.len() == 1);
    assert!(ret[0].data == vec![1, 2, 3]);
    assert_eq!(ret[0].order_channel, 7);
}

#[tokio::test]
async fn test_sendq() {
    let mut s = SendQ::new(1500);
    let p = FrameSetPacket::new(Reliability::Reliable, vec![]);
    s.insert(Reliability::Reliable, &p.serialize().unwrap())
        .unwrap();

    let p = FrameSetPacket::new(Reliability::Reliable, vec![]);
    s.insert(Reliability::Reliable, &p.serialize().unwrap())
        .unwrap();

    let sockaddr: SocketAddr = "127.0.0.1:8000".parse().unwrap();
    let ret = s.flush(0, &sockaddr);
    assert!(ret.len() == 2);

    s.ack(0, 0);
    s.ack(1, 0);

    let ret = s.flush(300, &sockaddr);
    assert!(ret.is_empty());
}

#[test]
fn send_queue_pipelines_reliable_packets_with_a_bounded_window() {
    let mut sendq = SendQ::new(1500);
    let peer = "127.0.0.1:8000".parse().unwrap();

    for index in 0..SendQ::MAX_IN_FLIGHT_PACKETS + 1 {
        sendq
            .insert(Reliability::ReliableOrdered, &[0xfe, index as u8])
            .unwrap();
        let sent = sendq.flush(0, &peer);
        if index < SendQ::MAX_IN_FLIGHT_PACKETS {
            assert_eq!(sent.len(), 1);
            assert_eq!(sent[0].ordered_frame_index, index as u32);
        } else {
            assert!(sent.is_empty());
        }
    }

    assert_eq!(sendq.get_sent_queue_size(), SendQ::MAX_IN_FLIGHT_PACKETS);
    assert_eq!(sendq.get_reliable_queue_size(), 1);

    sendq.ack(0, 1);
    let sent = sendq.flush(1, &peer);
    assert_eq!(sent.len(), 1);
    assert_eq!(
        sent[0].ordered_frame_index,
        SendQ::MAX_IN_FLIGHT_PACKETS as u32
    );
    assert_eq!(sendq.get_sent_queue_size(), SendQ::MAX_IN_FLIGHT_PACKETS);
    assert_eq!(sendq.get_reliable_queue_size(), 0);
}

#[test]
fn send_queue_retransmits_immediately_after_nack() {
    let mut sendq = SendQ::new(1500);
    let peer = "127.0.0.1:8000".parse().unwrap();
    sendq.insert(Reliability::Reliable, &[0xfe]).unwrap();

    let first = sendq.flush(0, &peer);
    assert_eq!(first.len(), 1);
    assert_eq!(first[0].sequence_number, 0);

    sendq.nack(0, 1);
    let retransmitted = sendq.flush(1, &peer);
    assert_eq!(retransmitted.len(), 1);
    assert_eq!(retransmitted[0].sequence_number, 1);
}

#[tokio::test]
async fn test_client_packet1() {
    let a = [
        140, 3, 0, 0, 112, 44, 192, 2, 0, 0, 1, 0, 0, 0, 0, 0, 0, 19, 0, 0, 0, 0, 0, 0, 254, 236,
        189, 203, 114, 234, 74, 183, 239, 185, 79, 175, 170, 30, 99, 159, 110, 125, 59, 36, 1, 222,
        166, 122, 19, 35, 97, 48, 18, 19, 161, 11, 82, 69, 197, 23, 128, 248, 38, 32, 9, 107, 218,
        152, 91, 69, 61, 79, 53, 234, 141, 78, 243, 52, 234, 5, 170, 119, 90, 149, 41, 165, 48, 41,
        11, 12, 2, 95, 214, 154, 255, 198, 47, 86, 120, 77, 15, 75, 202, 203, 24, 57, 46, 153, 249,
        223, 255, 199, 127, 253, 47, 255, 246, 111, 255, 229, 255, 253, 111, 255, 227, 191, 254,
        63, 255, 243, 191, 253, 219, 255, 249, 239, 163, 201, 96, 58, 255, 247, 255, 237, 127, 255,
        247, 241, 166, 53, 25, 54, 70, 211, 206, 180, 165, 152, 91, 181, 172, 221, 53, 159, 155,
        243, 95, 162, 215, 107, 222, 52, 125, 177, 105, 247, 61, 165, 107, 5, 205, 158, 84, 29,
        244, 250, 209, 111, 85, 214, 106, 166, 21, 52, 122, 91, 93, 209, 77, 165, 222, 51, 117,
        215, 21, 20, 197, 109, 148, 55, 166, 220, 122, 234, 216, 101, 105, 108, 43, 157, 241, 180,
        218, 49, 26, 250, 189, 165, 184, 243, 129, 242, 171, 50, 170, 7, 247, 109, 241, 113, 173,
        221, 235, 205, 225, 60, 18, 218, 226, 196, 234, 134, 186, 57, 240, 43, 83, 43, 112, 167,
        150, 57, 89, 13, 75, 90, 205, 146, 244, 123, 183, 46, 111, 199, 182, 176, 214, 182, 147,
        141, 214, 16, 187, 227, 134, 38, 247, 234, 150, 219, 107, 84, 231, 78, 201, 17, 186, 115,
        189, 174, 139, 154, 51, 82, 212, 210, 192, 178, 30, 13, 49, 216, 186, 179, 218, 203, 192,
        143, 238, 61, 177, 37, 186, 211, 234, 147, 41, 105, 91, 87, 169, 61, 142, 239, 22, 207,
        170, 28, 205, 199, 117, 253, 121, 32, 107, 117, 213, 119, 166, 255, 234, 62, 254, 7, 249,
        238, 153, 219, 111, 9, 3, 219, 141, 28, 73, 17, 92, 83, 17, 189, 198, 100, 57, 10, 3, 97,
        76, 190, 221, 187, 39, 127, 167, 183, 154, 186, 253, 201, 170, 57, 35, 239, 61, 235, 150,
        58, 117, 103, 173, 25, 242, 186, 125, 215, 138, 220, 134, 245, 226, 53, 200, 239, 90, 53,
        209, 9, 215, 145, 35, 44, 130, 241, 153, 109, 214, 145, 45, 117, 32, 5, 229, 177, 185, 158,
        121, 210, 122, 48, 154, 7, 150, 105, 107, 162, 106, 233, 146, 41, 87, 23, 61, 163, 117,
        167, 149, 92, 167, 83, 215, 94, 220, 70, 165, 111, 6, 214, 196, 110, 8, 37, 237, 222, 107,
        184, 161, 76, 191, 243, 217, 19, 149, 142, 213, 112, 55, 134, 226, 54, 29, 163, 213, 29,
        218, 214, 203, 72, 246, 90, 154, 31, 61, 246, 76, 209, 234, 133, 74, 223, 158, 183, 126,
        15, 77, 241, 119, 199, 168, 117, 134, 194, 162, 163, 7, 90, 167, 59, 183, 218, 110, 67, 40,
        143, 130, 32, 178, 239, 181, 208, 233, 63, 110, 123, 91, 85, 26, 223, 221, 174, 45, 163,
        41, 245, 238, 107, 247, 170, 18, 149, 123, 155, 170, 173, 90, 206, 148, 124, 243, 139, 19,
        58, 211, 206, 76, 150, 180, 250, 72, 234, 212, 127, 73, 154, 161, 84, 239, 126, 253, 199,
        63, 158, 123, 119, 119, 203, 126, 120, 247, 220, 113, 55, 245, 97, 189, 212, 172, 255, 136,
        30, 159, 164, 122, 255, 95, 245, 141, 220, 159, 41, 149, 169, 218, 173, 174, 5, 249, 247,
        83, 205, 188, 185, 81, 220, 113, 251, 229, 101, 213, 249, 245, 123, 212, 248, 71, 239, 105,
        48, 151, 68, 103, 32, 175, 215, 63, 220, 113, 216, 169, 252, 243, 247, 211, 118, 230, 255,
        179, 210, 173, 173, 123, 230, 203, 63, 170, 163, 127, 12, 52, 39, 26, 247, 158, 182, 147,
        113, 52, 154, 4, 75, 219, 122, 168, 223, 120, 255, 152, 223, 138, 173, 135, 122, 79, 43,
        245, 27, 255, 254, 191, 210, 81, 92, 214, 250, 38, 29, 197, 90, 79, 9, 74, 164, 149, 221,
        158, 188, 88, 218, 126, 112, 51, 152, 213, 234, 93, 50, 12, 117, 255, 89, 208, 77, 171,
        214, 21, 2, 217, 182, 189, 154, 110, 76, 20, 163, 177, 136, 198, 247, 129, 234, 148, 188,
        103, 210, 74, 21, 203, 82, 102, 158, 161, 152, 94, 80, 251, 57, 52, 253, 77, 151, 244, 139,
        93, 255, 181, 29, 134, 250, 139, 37, 182, 44, 199, 154, 44, 180, 173, 94, 210, 67, 119,
        162, 217, 238, 168, 45, 173, 75, 166, 89, 49, 221, 121, 75, 181, 252, 201, 68, 13, 106, 11,
        215, 174, 172, 116, 193, 151, 122, 225, 164, 101, 88, 214, 131, 99, 121, 131, 81, 24, 45,
        12, 251, 177, 162, 202, 149, 101, 79, 168, 52, 76, 161, 114, 231, 153, 11, 127, 104, 76,
        54, 182, 29, 56, 35, 201, 157, 140, 103, 90, 91, 221, 186, 229, 126, 182, 7, 196, 213, 210,
        154, 41, 118, 115, 186, 154, 58, 246, 122, 78, 70, 227, 84, 183, 212, 109, 167, 222, 37,
        35, 153, 14, 228, 156, 142, 145, 201, 36, 38, 19, 59, 212, 151, 67, 179, 178, 28, 134, 90,
        64, 196, 74, 234, 204, 41, 107, 51, 191, 172, 209, 137, 62, 107, 10, 228, 191, 2, 249, 125,
        129, 14, 232, 81, 73, 141, 155, 111, 40, 69, 68, 110, 68, 59, 60, 24, 223, 255, 72, 255,
        46, 249, 125, 89, 84, 233, 223, 37, 127, 107, 52, 215, 35, 55, 12, 102, 78, 95, 15, 186,
        125, 75, 24, 52, 170, 155, 65, 95, 175, 52, 103, 145, 48, 154, 91, 1, 253, 123, 78, 191,
        187, 255, 78, 165, 68, 54, 240, 73, 243, 8, 244, 119, 205, 123, 107, 58, 108, 4, 179, 158,
        100, 85, 232, 39, 25, 230, 196, 245, 4, 171, 102, 155, 147, 246, 80, 140, 90, 227, 240,
        113, 213, 21, 20, 221, 54, 221, 182, 38, 91, 74, 215, 212, 90, 186, 18, 204, 187, 166, 181,
        26, 26, 90, 93, 187, 91, 180, 212, 128, 40, 129, 198, 194, 232, 88, 171, 229, 80, 28, 109,
        92, 203, 43, 121, 115, 125, 221, 19, 162, 178, 102, 138, 174, 109, 77, 12, 215, 168, 217,
        186, 36, 151, 71, 164, 139, 61, 223, 26, 12, 132, 245, 114, 68, 148, 196, 64, 138, 106,
        166, 160, 77, 6, 210, 66, 232, 137, 122, 107, 228, 59, 226, 184, 111, 169, 35, 203, 85, 61,
        165, 86, 25, 247, 221, 126, 215, 94, 76, 186, 118, 180, 30, 207, 38, 91, 77, 209, 219, 61,
        203, 213, 186, 225, 164, 231, 206, 213, 167, 14, 153, 252, 142, 45, 170, 227, 198, 164, 57,
        188, 143, 30, 109, 197, 93, 147, 103, 205, 173, 251, 214, 202, 178, 221, 27, 39, 80, 250,
        134, 160, 210, 46, 50, 255, 53, 31, 119, 199, 195, 137, 182, 49, 141, 199, 40, 108, 191,
        244, 212, 222, 63, 198, 213, 255, 188, 239, 206, 102, 155, 223, 102, 235, 159, 255, 252,
        241, 16, 244, 194, 233, 68, 170, 7, 27, 189, 211, 250, 189, 104, 143, 255, 241, 159, 66,
        221, 121, 169, 73, 98, 189, 90, 107, 78, 23, 29, 229, 215, 96, 54, 27, 153, 183, 182, 223,
        155, 223, 55, 127, 24, 227, 219, 167, 217, 237, 188, 31, 74, 154, 226, 255, 115, 220, 169,
        143, 165, 231, 135, 95, 245, 217, 118, 174, 201, 134, 255, 88, 121, 92, 132, 129, 123, 251,
        175, 250, 204, 189, 169, 46, 138, 78, 144, 126, 109, 161, 10, 170, 240, 32, 248, 27, 83,
        156, 172, 6, 162, 90, 233, 223, 85, 151, 214, 182, 21, 88, 37, 79, 242, 238, 149, 118, 143,
        152, 10, 131, 232, 111, 91, 209, 2, 85, 113, 239, 29, 99, 178, 37, 170, 76, 210, 173, 232,
        201, 104, 84, 183, 230, 220, 122, 26, 248, 138, 209, 149, 148, 167, 65, 73, 111, 91, 50,
        249, 194, 153, 85, 241, 204, 245, 218, 242, 215, 130, 121, 31, 84, 188, 192, 171, 13, 36,
        165, 54, 152, 43, 55, 157, 123, 85, 176, 228, 69, 203, 242, 197, 187, 129, 210, 10, 71,
        155, 231, 50, 25, 45, 147, 161, 185, 46, 15, 228, 201, 243, 56, 156, 56, 214, 92, 9, 212,
        134, 103, 142, 230, 53, 203, 157, 71, 83, 211, 242, 126, 118, 55, 196,
    ];

    let b = FrameVec::new(&a).unwrap();
    assert!(b.frames.len() == 1);
    assert!(b.frames[0].is_fragment());
}

#[tokio::test]
async fn test_client_packet2() {
    // Connection Request - reliable [ reliable_frame_index = 0 ]
    let p0 = [
        132, 0, 0, 0, 64, 0, 144, 0, 0, 0, 9, 162, 70, 235, 28, 218, 182, 26, 192, 0, 0, 0, 0, 16,
        151, 43, 113, 0,
    ];
    // Connection Request - reliable [ reliable_frame_index = 0 ]
    let p1 = [
        132, 1, 0, 0, 64, 0, 144, 0, 0, 0, 9, 162, 70, 235, 28, 218, 182, 26, 192, 0, 0, 0, 0, 16,
        151, 43, 113, 0,
    ];
    // 2 frames Incompatible Protocol(extract data?) Connected ping - reliable ordered [ reliable_frame_index = 1 ]
    let p2 = [
        132, 2, 0, 0, 96, 9, 64, 1, 0, 0, 0, 0, 0, 0, 19, 4, 83, 237, 234, 82, 74, 188, 6, 23, 0,
        225, 138, 0, 0, 0, 0, 254, 128, 0, 0, 0, 0, 0, 0, 196, 178, 112, 86, 5, 59, 97, 219, 15, 0,
        0, 0, 6, 23, 0, 225, 138, 0, 0, 0, 0, 254, 128, 0, 0, 0, 0, 0, 0, 188, 210, 59, 150, 246,
        167, 182, 213, 33, 0, 0, 0, 6, 23, 0, 225, 138, 0, 0, 0, 0, 254, 128, 0, 0, 0, 0, 0, 0,
        132, 194, 47, 23, 175, 46, 78, 138, 23, 0, 0, 0, 6, 23, 0, 225, 138, 0, 0, 0, 0, 254, 128,
        0, 0, 0, 0, 0, 0, 136, 219, 85, 240, 191, 125, 172, 233, 10, 0, 0, 0, 6, 23, 0, 225, 138,
        0, 0, 0, 0, 254, 128, 0, 0, 0, 0, 0, 0, 80, 211, 212, 44, 191, 227, 124, 40, 13, 0, 0, 0,
        6, 23, 0, 225, 138, 0, 0, 0, 0, 254, 128, 0, 0, 0, 0, 0, 0, 77, 13, 19, 149, 102, 140, 134,
        77, 16, 0, 0, 0, 4, 83, 237, 239, 254, 225, 138, 4, 63, 87, 214, 254, 225, 138, 4, 83, 236,
        159, 254, 225, 138, 4, 63, 87, 46, 254, 225, 138, 4, 63, 87, 56, 128, 225, 138, 4, 63, 87,
        56, 123, 225, 138, 4, 255, 255, 255, 255, 0, 0, 4, 255, 255, 255, 255, 0, 0, 4, 255, 255,
        255, 255, 0, 0, 4, 255, 255, 255, 255, 0, 0, 4, 255, 255, 255, 255, 0, 0, 4, 255, 255, 255,
        255, 0, 0, 4, 255, 255, 255, 255, 0, 0, 4, 255, 255, 255, 255, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        0, 0, 0, 0, 0, 16, 151, 56, 146, 0, 0, 72, 0, 0, 0, 0, 0, 16, 151, 56, 146,
    ];
    // 1 frames Connected ping - unreliable
    let p3 = [132, 3, 0, 0, 0, 0, 72, 0, 0, 0, 0, 0, 16, 151, 56, 161];
    // 1 frames game packet - reliable ordered [is_fragment reliable_frame_index = 2 ordered_frame_index = 1 ]
    let p4 = [
        140, 4, 0, 0, 112, 44, 192, 2, 0, 0, 1, 0, 0, 0, 0, 0, 0, 19, 0, 0, 0, 0, 0, 0, 254, 236,
        189, 203, 114, 234, 202, 214, 239, 249, 157, 94, 85, 61, 198, 119, 186, 181, 79, 72, 2,
        188, 77, 245, 140, 145, 48, 24, 137, 137, 208, 5, 169, 162, 98, 7, 32, 246, 4, 36, 97, 77,
        155, 201, 173, 162, 158, 231, 188, 81, 53, 171, 89, 47, 80, 189, 211, 170, 148, 148, 194,
        164, 156, 96, 16, 248, 178, 214, 252, 55, 126, 177, 194, 107, 122, 88, 82, 94, 198, 200,
        113, 201, 204, 255, 231, 127, 252, 215, 255, 242, 31, 255, 241, 95, 254, 223, 255, 251,
        127, 252, 215, 255, 254, 63, 255, 199, 127, 252, 159, 255, 57, 154, 12, 166, 243, 255, 252,
        223, 254, 247, 255, 28, 111, 90, 147, 97, 99, 52, 237, 76, 91, 138, 185, 85, 203, 218, 125,
        243, 165, 57, 255, 41, 122, 189, 230, 77, 211, 23, 155, 118, 223, 83, 186, 86, 208, 236,
        73, 213, 65, 175, 31, 253, 82, 101, 173, 102, 90, 65, 163, 183, 213, 21, 221, 84, 234, 61,
        83, 119, 93, 65, 81, 172, 70, 107, 233, 25, 138, 210, 179, 149, 23, 79, 18, 205, 142, 41,
        8, 163, 134, 179, 177, 77, 87, 182, 131, 86, 121, 80, 106, 213, 135, 125, 179, 162, 139,
        122, 203, 241, 131, 138, 109, 4, 93, 51, 168, 173, 135, 74, 75, 29, 214, 29, 97, 104, 4,
        55, 222, 182, 86, 26, 205, 189, 242, 192, 127, 217, 90, 219, 174, 48, 40, 233, 115, 171,
        49, 105, 141, 109, 189, 63, 182, 91, 63, 70, 141, 167, 237, 120, 166, 148, 213, 173, 92,
        30, 110, 149, 78, 91, 12, 66, 93, 241, 5, 93, 208, 45, 45, 156, 56, 143, 130, 18, 90, 194,
        147, 104, 55, 116, 209, 8, 163, 103, 211, 94, 151, 134, 91, 189, 50, 174, 235, 191, 135,
        138, 213, 237, 218, 21, 211, 182, 252, 233, 191, 187, 79, 255, 141, 124, 247, 204, 237,
        183, 132, 129, 237, 70, 142, 164, 8, 174, 169, 136, 94, 99, 178, 28, 133, 129, 48, 38, 223,
        238, 61, 180, 68, 183, 183, 154, 186, 253, 201, 170, 57, 123, 90, 107, 179, 110, 169, 83,
        119, 214, 154, 33, 175, 219, 247, 173, 200, 109, 88, 191, 189, 6, 249, 93, 171, 38, 58,
        225, 58, 114, 132, 69, 48, 62, 179, 205, 58, 178, 165, 14, 164, 160, 60, 54, 215, 51, 79,
        90, 15, 70, 243, 192, 50, 109, 77, 84, 45, 93, 50, 229, 234, 162, 103, 180, 238, 181, 146,
        235, 116, 234, 218, 111, 183, 81, 233, 155, 129, 53, 177, 27, 66, 73, 123, 240, 26, 110,
        40, 139, 238, 180, 250, 226, 137, 74, 199, 106, 184, 27, 67, 113, 155, 142, 209, 234, 14,
        109, 235, 247, 72, 246, 90, 154, 31, 61, 245, 76, 209, 234, 133, 74, 223, 158, 183, 126,
        13, 77, 241, 87, 199, 168, 117, 134, 194, 162, 163, 7, 90, 167, 59, 183, 218, 110, 67, 40,
        143, 130, 32, 178, 31, 180, 208, 233, 63, 109, 123, 91, 85, 26, 223, 223, 174, 45, 163, 41,
        245, 30, 106, 15, 170, 18, 149, 123, 155, 170, 173, 90, 206, 148, 124, 243, 111, 39, 116,
        166, 157, 153, 44, 105, 245, 145, 212, 169, 255, 148, 52, 67, 169, 222, 255, 252, 111, 237,
        167, 127, 74, 191, 234, 254, 100, 169, 140, 42, 171, 201, 92, 173, 142, 252, 127, 77, 86,
        247, 77, 251, 95, 254, 175, 127, 215, 127, 223, 246, 42, 255, 26, 253, 235, 165, 108, 133,
        255, 246, 151, 203, 202, 211, 250, 151, 175, 12, 203, 193, 124, 221, 155, 255, 67, 126,
        190, 109, 253, 99, 242, 243, 95, 211, 167, 219, 187, 237, 157, 107, 254, 43, 24, 107, 81,
        79, 22, 171, 225, 38, 252, 183, 240, 24, 4, 255, 114, 31, 21, 213, 235, 11, 195, 81, 239,
        159, 253, 161, 97, 169, 222, 77, 215, 41, 173, 164, 231, 254, 68, 157, 151, 199, 131, 31,
        15, 51, 229, 63, 255, 215, 120, 20, 151, 181, 190, 25, 143, 98, 173, 167, 4, 37, 210, 202,
        110, 79, 94, 44, 109, 63, 184, 25, 204, 106, 245, 174, 165, 184, 186, 255, 34, 232, 166,
        85, 235, 10, 129, 108, 219, 94, 77, 55, 38, 138, 209, 88, 68, 227, 135, 64, 117, 74, 222,
        11, 105, 165, 138, 101, 41, 51, 50, 138, 77, 47, 168, 253, 24, 154, 254, 166, 75, 250, 197,
        174, 255, 220, 14, 67, 253, 183, 37, 182, 44, 199, 154, 44, 180, 173, 94, 210, 67, 119,
        162, 217, 238, 168, 45, 173, 75, 166, 89, 49, 221, 121, 75, 181, 252, 201, 68, 13, 106, 11,
        215, 174, 172, 116, 193, 151, 122, 225, 164, 101, 88, 214, 163, 99, 121, 131, 81, 24, 45,
        12, 251, 169, 162, 202, 149, 101, 79, 168, 52, 76, 161, 114, 239, 153, 11, 127, 104, 76,
        54, 182, 29, 56, 35, 201, 157, 140, 103, 90, 91, 221, 186, 229, 126, 190, 7, 196, 213, 210,
        154, 41, 118, 115, 186, 154, 58, 246, 122, 78, 70, 227, 84, 183, 212, 109, 167, 222, 37,
        35, 57, 30, 200, 156, 142, 145, 201, 36, 38, 19, 59, 212, 151, 67, 179, 178, 28, 134, 90,
        64, 196, 74, 234, 204, 41, 107, 51, 191, 172, 197, 19, 125, 214, 20, 200, 127, 5, 242, 251,
        66, 60, 160, 71, 37, 53, 105, 190, 161, 20, 17, 185, 81, 220, 225, 193, 248, 225, 46, 251,
        187, 228, 247, 101, 81, 141, 255, 46, 249, 91, 163, 185, 30, 185, 97, 48, 115, 250, 122,
        208, 237, 91, 194, 160, 81, 221, 12, 250, 122, 165, 57, 139, 132, 209, 220, 10, 226, 191,
        231, 244, 187, 251, 239, 84, 74, 101, 3, 159, 52, 143, 16, 255, 174, 249, 96, 77, 135, 141,
        96, 214, 147, 172, 74, 252, 73, 134, 57, 113, 61, 193, 170, 217, 230, 164, 61, 20, 163,
        214, 56, 124, 90, 117, 5, 69, 39, 10, 164, 173, 201, 150, 210, 53, 181, 150, 174, 4, 243,
        174, 105, 173, 134, 134, 86, 215, 238, 23, 45, 53, 208, 156, 81, 99, 97, 116, 172, 213,
        114, 40, 142, 54, 174, 229, 149, 188, 185, 190, 238, 9, 81, 89, 51, 69, 215, 182, 38, 134,
        107, 212, 108, 93, 146, 203, 35, 210, 197, 158, 111, 13, 6, 194, 122, 57, 18, 91, 226, 64,
        138, 106, 166, 160, 77, 6, 210, 66, 232, 17, 165, 52, 242, 29, 113, 220, 183, 212, 145,
        229, 170, 158, 82, 171, 140, 251, 110, 191, 107, 47, 38, 93, 59, 90, 143, 103, 147, 173,
        166, 232, 237, 158, 229, 106, 221, 112, 210, 115, 231, 234, 115, 135, 76, 126, 199, 22,
        213, 113, 99, 210, 28, 62, 68, 79, 182, 226, 174, 201, 179, 230, 214, 67, 107, 101, 217,
        238, 141, 19, 40, 125, 67, 80, 227, 46, 50, 255, 61, 31, 119, 199, 195, 137, 182, 49, 141,
        167, 40, 108, 255, 238, 169, 189, 127, 140, 171, 255, 124, 232, 206, 102, 155, 95, 102,
        235, 95, 255, 186, 123, 12, 122, 225, 116, 34, 213, 131, 141, 222, 105, 253, 90, 180, 199,
        255, 248, 167, 80, 119, 126, 215, 36, 177, 94, 173, 53, 167, 139, 142, 242, 115, 48, 155,
        141, 204, 91, 219, 239, 205, 31, 154, 119, 198, 248, 246, 121, 118, 59, 239, 135, 146, 166,
        248, 255, 26, 119, 234, 99, 233, 229, 241, 103, 125, 182, 157, 107, 178, 225, 63, 85, 158,
        22, 97, 224, 222, 254, 187, 62, 115, 111, 170, 139, 162, 19, 164, 95, 91, 168, 130, 42, 60,
        10, 254, 198, 20, 39, 171, 129, 168, 86, 250, 247, 213, 165, 181, 109, 5, 86, 201, 147,
        188, 7, 165, 221, 35, 166, 194, 176, 2, 215, 86, 180, 64, 85, 220, 7, 199, 152, 108, 137,
        42, 147, 116, 43, 122, 54, 26, 213, 173, 57, 183, 158, 7, 190, 98, 116, 37, 229, 153, 168,
        243, 182, 37, 147, 47, 156, 89, 21, 207, 92, 175, 45, 127, 45, 152, 15, 65, 197, 11, 188,
        218, 64, 82, 106, 131, 185, 114, 211, 121, 80, 5, 75, 94, 180, 44, 95, 188, 31, 40, 173,
        112, 180, 121, 41, 147, 209, 50, 25, 154, 235, 242, 64, 158, 188, 140, 137, 154, 183, 230,
        74, 160, 54, 60, 115, 52, 175, 89, 238, 60, 154, 154, 150, 247, 163, 187,
    ];
    // 1 frames Incompatible Protocol(extract data?) - reliable ordered [reliable_frame_index = 1 ordered_frame_index = 0] == p2
    let p5 = [
        140, 5, 0, 0, 96, 9, 64, 1, 0, 0, 0, 0, 0, 0, 19, 4, 83, 237, 234, 82, 74, 188, 6, 23, 0,
        225, 138, 0, 0, 0, 0, 254, 128, 0, 0, 0, 0, 0, 0, 196, 178, 112, 86, 5, 59, 97, 219, 15, 0,
        0, 0, 6, 23, 0, 225, 138, 0, 0, 0, 0, 254, 128, 0, 0, 0, 0, 0, 0, 188, 210, 59, 150, 246,
        167, 182, 213, 33, 0, 0, 0, 6, 23, 0, 225, 138, 0, 0, 0, 0, 254, 128, 0, 0, 0, 0, 0, 0,
        132, 194, 47, 23, 175, 46, 78, 138, 23, 0, 0, 0, 6, 23, 0, 225, 138, 0, 0, 0, 0, 254, 128,
        0, 0, 0, 0, 0, 0, 136, 219, 85, 240, 191, 125, 172, 233, 10, 0, 0, 0, 6, 23, 0, 225, 138,
        0, 0, 0, 0, 254, 128, 0, 0, 0, 0, 0, 0, 80, 211, 212, 44, 191, 227, 124, 40, 13, 0, 0, 0,
        6, 23, 0, 225, 138, 0, 0, 0, 0, 254, 128, 0, 0, 0, 0, 0, 0, 77, 13, 19, 149, 102, 140, 134,
        77, 16, 0, 0, 0, 4, 83, 237, 239, 254, 225, 138, 4, 63, 87, 214, 254, 225, 138, 4, 83, 236,
        159, 254, 225, 138, 4, 63, 87, 46, 254, 225, 138, 4, 63, 87, 56, 128, 225, 138, 4, 63, 87,
        56, 123, 225, 138, 4, 255, 255, 255, 255, 0, 0, 4, 255, 255, 255, 255, 0, 0, 4, 255, 255,
        255, 255, 0, 0, 4, 255, 255, 255, 255, 0, 0, 4, 255, 255, 255, 255, 0, 0, 4, 255, 255, 255,
        255, 0, 0, 4, 255, 255, 255, 255, 0, 0, 4, 255, 255, 255, 255, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        0, 0, 0, 0, 0, 16, 151, 56, 146,
    ];
    // 1 frames game packet - reliable ordered [is_fragment reliable_frame_index = 2 ordered_frame_index = 1 ] == p4
    let p6 = [
        140, 6, 0, 0, 112, 44, 192, 2, 0, 0, 1, 0, 0, 0, 0, 0, 0, 19, 0, 0, 0, 0, 0, 0, 254, 236,
        189, 203, 114, 234, 202, 214, 239, 249, 157, 94, 85, 61, 198, 119, 186, 181, 79, 72, 2,
        188, 77, 245, 140, 145, 48, 24, 137, 137, 208, 5, 169, 162, 98, 7, 32, 246, 4, 36, 97, 77,
        155, 201, 173, 162, 158, 231, 188, 81, 53, 171, 89, 47, 80, 189, 211, 170, 148, 148, 194,
        164, 156, 96, 16, 248, 178, 214, 252, 55, 126, 177, 194, 107, 122, 88, 82, 94, 198, 200,
        113, 201, 204, 255, 231, 127, 252, 215, 255, 242, 31, 255, 241, 95, 254, 223, 255, 251,
        127, 252, 215, 255, 254, 63, 255, 199, 127, 252, 159, 255, 57, 154, 12, 166, 243, 255, 252,
        223, 254, 247, 255, 28, 111, 90, 147, 97, 99, 52, 237, 76, 91, 138, 185, 85, 203, 218, 125,
        243, 165, 57, 255, 41, 122, 189, 230, 77, 211, 23, 155, 118, 223, 83, 186, 86, 208, 236,
        73, 213, 65, 175, 31, 253, 82, 101, 173, 102, 90, 65, 163, 183, 213, 21, 221, 84, 234, 61,
        83, 119, 93, 65, 81, 172, 70, 107, 233, 25, 138, 210, 179, 149, 23, 79, 18, 205, 142, 41,
        8, 163, 134, 179, 177, 77, 87, 182, 131, 86, 121, 80, 106, 213, 135, 125, 179, 162, 139,
        122, 203, 241, 131, 138, 109, 4, 93, 51, 168, 173, 135, 74, 75, 29, 214, 29, 97, 104, 4,
        55, 222, 182, 86, 26, 205, 189, 242, 192, 127, 217, 90, 219, 174, 48, 40, 233, 115, 171,
        49, 105, 141, 109, 189, 63, 182, 91, 63, 70, 141, 167, 237, 120, 166, 148, 213, 173, 92,
        30, 110, 149, 78, 91, 12, 66, 93, 241, 5, 93, 208, 45, 45, 156, 56, 143, 130, 18, 90, 194,
        147, 104, 55, 116, 209, 8, 163, 103, 211, 94, 151, 134, 91, 189, 50, 174, 235, 191, 135,
        138, 213, 237, 218, 21, 211, 182, 252, 233, 191, 187, 79, 255, 141, 124, 247, 204, 237,
        183, 132, 129, 237, 70, 142, 164, 8, 174, 169, 136, 94, 99, 178, 28, 133, 129, 48, 38, 223,
        238, 61, 180, 68, 183, 183, 154, 186, 253, 201, 170, 57, 123, 90, 107, 179, 110, 169, 83,
        119, 214, 154, 33, 175, 219, 247, 173, 200, 109, 88, 191, 189, 6, 249, 93, 171, 38, 58,
        225, 58, 114, 132, 69, 48, 62, 179, 205, 58, 178, 165, 14, 164, 160, 60, 54, 215, 51, 79,
        90, 15, 70, 243, 192, 50, 109, 77, 84, 45, 93, 50, 229, 234, 162, 103, 180, 238, 181, 146,
        235, 116, 234, 218, 111, 183, 81, 233, 155, 129, 53, 177, 27, 66, 73, 123, 240, 26, 110,
        40, 139, 238, 180, 250, 226, 137, 74, 199, 106, 184, 27, 67, 113, 155, 142, 209, 234, 14,
        109, 235, 247, 72, 246, 90, 154, 31, 61, 245, 76, 209, 234, 133, 74, 223, 158, 183, 126,
        13, 77, 241, 87, 199, 168, 117, 134, 194, 162, 163, 7, 90, 167, 59, 183, 218, 110, 67, 40,
        143, 130, 32, 178, 31, 180, 208, 233, 63, 109, 123, 91, 85, 26, 223, 223, 174, 45, 163, 41,
        245, 30, 106, 15, 170, 18, 149, 123, 155, 170, 173, 90, 206, 148, 124, 243, 111, 39, 116,
        166, 157, 153, 44, 105, 245, 145, 212, 169, 255, 148, 52, 67, 169, 222, 255, 252, 111, 237,
        167, 127, 74, 191, 234, 254, 100, 169, 140, 42, 171, 201, 92, 173, 142, 252, 127, 77, 86,
        247, 77, 251, 95, 254, 175, 127, 215, 127, 223, 246, 42, 255, 26, 253, 235, 165, 108, 133,
        255, 246, 151, 203, 202, 211, 250, 151, 175, 12, 203, 193, 124, 221, 155, 255, 67, 126,
        190, 109, 253, 99, 242, 243, 95, 211, 167, 219, 187, 237, 157, 107, 254, 43, 24, 107, 81,
        79, 22, 171, 225, 38, 252, 183, 240, 24, 4, 255, 114, 31, 21, 213, 235, 11, 195, 81, 239,
        159, 253, 161, 97, 169, 222, 77, 215, 41, 173, 164, 231, 254, 68, 157, 151, 199, 131, 31,
        15, 51, 229, 63, 255, 215, 120, 20, 151, 181, 190, 25, 143, 98, 173, 167, 4, 37, 210, 202,
        110, 79, 94, 44, 109, 63, 184, 25, 204, 106, 245, 174, 165, 184, 186, 255, 34, 232, 166,
        85, 235, 10, 129, 108, 219, 94, 77, 55, 38, 138, 209, 88, 68, 227, 135, 64, 117, 74, 222,
        11, 105, 165, 138, 101, 41, 51, 50, 138, 77, 47, 168, 253, 24, 154, 254, 166, 75, 250, 197,
        174, 255, 220, 14, 67, 253, 183, 37, 182, 44, 199, 154, 44, 180, 173, 94, 210, 67, 119,
        162, 217, 238, 168, 45, 173, 75, 166, 89, 49, 221, 121, 75, 181, 252, 201, 68, 13, 106, 11,
        215, 174, 172, 116, 193, 151, 122, 225, 164, 101, 88, 214, 163, 99, 121, 131, 81, 24, 45,
        12, 251, 169, 162, 202, 149, 101, 79, 168, 52, 76, 161, 114, 239, 153, 11, 127, 104, 76,
        54, 182, 29, 56, 35, 201, 157, 140, 103, 90, 91, 221, 186, 229, 126, 190, 7, 196, 213, 210,
        154, 41, 118, 115, 186, 154, 58, 246, 122, 78, 70, 227, 84, 183, 212, 109, 167, 222, 37,
        35, 57, 30, 200, 156, 142, 145, 201, 36, 38, 19, 59, 212, 151, 67, 179, 178, 28, 134, 90,
        64, 196, 74, 234, 204, 41, 107, 51, 191, 172, 197, 19, 125, 214, 20, 200, 127, 5, 242, 251,
        66, 60, 160, 71, 37, 53, 105, 190, 161, 20, 17, 185, 81, 220, 225, 193, 248, 225, 46, 251,
        187, 228, 247, 101, 81, 141, 255, 46, 249, 91, 163, 185, 30, 185, 97, 48, 115, 250, 122,
        208, 237, 91, 194, 160, 81, 221, 12, 250, 122, 165, 57, 139, 132, 209, 220, 10, 226, 191,
        231, 244, 187, 251, 239, 84, 74, 101, 3, 159, 52, 143, 16, 255, 174, 249, 96, 77, 135, 141,
        96, 214, 147, 172, 74, 252, 73, 134, 57, 113, 61, 193, 170, 217, 230, 164, 61, 20, 163,
        214, 56, 124, 90, 117, 5, 69, 39, 10, 164, 173, 201, 150, 210, 53, 181, 150, 174, 4, 243,
        174, 105, 173, 134, 134, 86, 215, 238, 23, 45, 53, 208, 156, 81, 99, 97, 116, 172, 213,
        114, 40, 142, 54, 174, 229, 149, 188, 185, 190, 238, 9, 81, 89, 51, 69, 215, 182, 38, 134,
        107, 212, 108, 93, 146, 203, 35, 210, 197, 158, 111, 13, 6, 194, 122, 57, 18, 91, 226, 64,
        138, 106, 166, 160, 77, 6, 210, 66, 232, 17, 165, 52, 242, 29, 113, 220, 183, 212, 145,
        229, 170, 158, 82, 171, 140, 251, 110, 191, 107, 47, 38, 93, 59, 90, 143, 103, 147, 173,
        166, 232, 237, 158, 229, 106, 221, 112, 210, 115, 231, 234, 115, 135, 76, 126, 199, 22,
        213, 113, 99, 210, 28, 62, 68, 79, 182, 226, 174, 201, 179, 230, 214, 67, 107, 101, 217,
        238, 141, 19, 40, 125, 67, 80, 227, 46, 50, 255, 61, 31, 119, 199, 195, 137, 182, 49, 141,
        167, 40, 108, 255, 238, 169, 189, 127, 140, 171, 255, 124, 232, 206, 102, 155, 95, 102,
        235, 95, 255, 186, 123, 12, 122, 225, 116, 34, 213, 131, 141, 222, 105, 253, 90, 180, 199,
        255, 248, 167, 80, 119, 126, 215, 36, 177, 94, 173, 53, 167, 139, 142, 242, 115, 48, 155,
        141, 204, 91, 219, 239, 205, 31, 154, 119, 198, 248, 246, 121, 118, 59, 239, 135, 146, 166,
        248, 255, 26, 119, 234, 99, 233, 229, 241, 103, 125, 182, 157, 107, 178, 225, 63, 85, 158,
        22, 97, 224, 222, 254, 187, 62, 115, 111, 170, 139, 162, 19, 164, 95, 91, 168, 130, 42, 60,
        10, 254, 198, 20, 39, 171, 129, 168, 86, 250, 247, 213, 165, 181, 109, 5, 86, 201, 147,
        188, 7, 165, 221, 35, 166, 194, 176, 2, 215, 86, 180, 64, 85, 220, 7, 199, 152, 108, 137,
        42, 147, 116, 43, 122, 54, 26, 213, 173, 57, 183, 158, 7, 190, 98, 116, 37, 229, 153, 168,
        243, 182, 37, 147, 47, 156, 89, 21, 207, 92, 175, 45, 127, 45, 152, 15, 65, 197, 11, 188,
        218, 64, 82, 106, 131, 185, 114, 211, 121, 80, 5, 75, 94, 180, 44, 95, 188, 31, 40, 173,
        112, 180, 121, 41, 147, 209, 50, 25, 154, 235, 242, 64, 158, 188, 140, 137, 154, 183, 230,
        74, 160, 54, 60, 115, 52, 175, 89, 238, 60, 154, 154, 150, 247, 163, 187,
    ];
    let ps: Vec<Vec<u8>> = vec![
        p0.to_vec(),
        p1.to_vec(),
        p2.to_vec(),
        p3.to_vec(),
        p4.to_vec(),
        p5.to_vec(),
        p6.to_vec(),
    ];

    let mut n = 0;

    let mut rq = RecvQ::new();
    for i in ps {
        let v = FrameVec::new(&i).unwrap();
        for i in v.frames {
            rq.insert(i).unwrap();
            if !rq.flush(&"0.0.0.0:0".parse().unwrap()).is_empty() {
                n += 1;
            }
        }
    }

    // p1 retransmits p0 with a new datagram ID but the same reliable frame ID.
    assert_eq!(n, 4);
}

#[cfg(test)]
#[path = "arq_tests.rs"]
mod regressions;

#[test]
fn rejects_messages_exceeding_the_receivers_fragment_limit() {
    let sendq = SendQ::new(576);
    let limit = crate::fragment::MAX_FRAGMENTS * (576 - 60);
    assert!(
        sendq
            .required_bytes(Reliability::ReliableOrdered, limit)
            .is_ok()
    );
    assert!(matches!(
        sendq.required_bytes(Reliability::ReliableOrdered, limit + 1),
        Err(RaknetError::PacketSizeExceedMTU)
    ));
}

#[test]
fn exact_mtu_payload_is_not_marked_as_split() {
    let mut sendq = SendQ::new(576);
    let payload = vec![0xfe; 516];
    sendq
        .insert(Reliability::ReliableOrdered, &payload)
        .unwrap();
    let frames = sendq.flush(0, &"127.0.0.1:19132".parse().unwrap());
    assert_eq!(frames.len(), 1);
    assert_eq!(frames[0].flags & 16, 0);
    assert_eq!(frames[0].data.as_ref(), payload);
    sendq.ack(frames[0].sequence_number, 1);
    assert!(sendq.is_empty());
    assert_eq!(sendq.buffered_bytes, 0);
}
