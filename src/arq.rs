use crate::sequence::{self, ReliableWindow};

use std::{
    collections::{HashMap, VecDeque},
    net::SocketAddr,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

use crate::{datatype::*, error::*, fragment::FragmentQ, raknet_log_debug, utils::*};

pub(crate) struct OutgoingFrames {
    frames: Vec<FrameSetPacket>,
    pub(crate) coalesced: bool,
}

impl OutgoingFrames {
    #[cfg(test)]
    pub(crate) fn new() -> Self {
        Self::with_capacity(0)
    }

    fn with_capacity(capacity: usize) -> Self {
        Self {
            frames: Vec::with_capacity(capacity),
            coalesced: false,
        }
    }
}

impl std::ops::Deref for OutgoingFrames {
    type Target = Vec<FrameSetPacket>;
    fn deref(&self) -> &Self::Target {
        &self.frames
    }
}

impl std::ops::DerefMut for OutgoingFrames {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.frames
    }
}

impl IntoIterator for OutgoingFrames {
    type Item = FrameSetPacket;
    type IntoIter = std::vec::IntoIter<FrameSetPacket>;
    fn into_iter(self) -> Self::IntoIter {
        self.frames.into_iter()
    }
}

impl<'a> IntoIterator for &'a OutgoingFrames {
    type Item = &'a FrameSetPacket;
    type IntoIter = std::slice::Iter<'a, FrameSetPacket>;
    fn into_iter(self) -> Self::IntoIter {
        self.frames.iter()
    }
}

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

// Received payloads move into the application without another allocation.
// Reliable sends share their backing allocation with retransmission frames.
#[derive(Clone, Debug)]
pub(crate) enum FramePayload {
    Owned(Vec<u8>),
    Bytes(bytes::Bytes),
    Shared {
        owner: Arc<[u8]>,
        start: u32,
        length: u16,
    },
}

impl Default for FramePayload {
    fn default() -> Self {
        Self::Owned(Vec::new())
    }
}

impl FramePayload {
    fn copy_shared(data: &[u8]) -> Self {
        Self::Shared {
            owner: Arc::from(data),
            start: 0,
            length: data.len() as u16,
        }
    }
}

impl std::ops::Deref for FramePayload {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        match self {
            Self::Owned(data) => data,
            Self::Bytes(data) => data,
            Self::Shared {
                owner,
                start,
                length,
            } => &owner[*start as usize..*start as usize + usize::from(*length)],
        }
    }
}

impl AsRef<[u8]> for FramePayload {
    fn as_ref(&self) -> &[u8] {
        self
    }
}

impl From<Vec<u8>> for FramePayload {
    fn from(data: Vec<u8>) -> Self {
        Self::Owned(data)
    }
}

impl From<FramePayload> for Vec<u8> {
    fn from(data: FramePayload) -> Self {
        match data {
            FramePayload::Owned(data) => data,
            shared => shared.to_vec(),
        }
    }
}

impl PartialEq for FramePayload {
    fn eq(&self, other: &Self) -> bool {
        self.as_ref() == other.as_ref()
    }
}

// A fragmented send retains its complete allocation until the final fragment
// is acknowledged. Charge that allocation once, independently of frame clones.
struct SharedPayload {
    remaining: AtomicUsize,
    bytes: usize,
}

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
    pub(crate) data: FramePayload,
    shared_payload: Option<Arc<SharedPayload>>,
}

impl FrameSetPacket {
    pub fn new(r: Reliability, data: Vec<u8>) -> Self {
        Self::from_payload(r, data.into())
    }

    fn from_payload(r: Reliability, data: FramePayload) -> Self {
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
            data,
            shared_payload: None,
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
            data: FramePayload::default(),
            shared_payload: None,
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
        let mut buffer = Vec::new();
        self.serialize_into(&mut buffer)?;
        Ok(buffer)
    }

    pub(crate) fn serialize_into(&self, buffer: &mut Vec<u8>) -> Result<()> {
        self.validate()?;
        let size = self._size()?;
        buffer.clear();
        buffer.reserve(size);
        let mut writer = RaknetWriter::from_buffer(std::mem::take(buffer));
        let result = self
            .write_header(&mut writer)
            .and_then(|()| writer.write(self.data.as_ref()));
        *buffer = writer.get_raw_payload();
        result
    }

    pub(crate) fn serialize_group_into(frames: &[Self], output: &mut Vec<u8>) -> Result<()> {
        let first = frames.first().ok_or(RaknetError::PacketParseError)?;
        output.clear();
        for (index, frame) in frames.iter().enumerate() {
            if frame.sequence_number != first.sequence_number {
                return Err(RaknetError::PacketParseError);
            }
            let mut header = [0; 32];
            let length = frame.encode_header(&mut header)?;
            output.extend_from_slice(&header[if index == 0 { 0 } else { 4 }..length]);
            output.extend_from_slice(frame.data.as_ref());
        }
        Ok(())
    }

    pub(crate) fn validate(&self) -> Result<()> {
        if self.data.len() != usize::from(self.length_in_bytes)
            || self.data.len() > usize::from(u16::MAX / 8)
            || (self.is_fragment()
                && (self.compound_size == 0 || self.fragment_index >= self.compound_size))
        {
            return Err(RaknetError::PacketParseError);
        }

        self._size()?;
        Ok(())
    }

    pub(crate) fn encode_header(&self, header: &mut [u8; 32]) -> Result<usize> {
        self.validate()?;
        let mut writer = RaknetWriter::from_buffer(header.as_mut_slice());
        self.write_header(&mut writer)?;
        Ok(32 - writer.remaining_mut())
    }

    fn write_header<B: bytes::BufMut>(&self, writer: &mut RaknetWriter<B>) -> Result<()> {
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
        Ok(())
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
    #[cfg(test)]
    pub frames: Vec<FrameSetPacket>,
}

impl FrameVec {
    #[cfg(test)]
    pub fn new(buf: &[u8]) -> Result<Self> {
        let mut frames = Vec::new();
        Self::decode_into(buf, &mut frames)?;
        Ok(Self { frames })
    }

    // Reuse the frame metadata allocation across incoming datagrams.
    pub(crate) fn decode_into(buf: &[u8], frames: &mut Vec<FrameSetPacket>) -> Result<()> {
        Self::decode(buf, frames, false)
    }

    pub(crate) fn decode_owned_into(
        mut buf: Vec<u8>,
        frames: &mut Vec<FrameSetPacket>,
    ) -> Result<()> {
        Self::decode(&buf, frames, true)?;
        if let [frame] = frames.as_mut_slice() {
            // A single-frame datagram can reuse its allocation for the payload.
            // Compact the body in place without a shared slice owner.
            let length = usize::from(frame.length_in_bytes);
            let offset = buf.len() - length;
            buf.copy_within(offset.., 0);
            buf.truncate(length);
            frame.data = buf.into();
        }
        Ok(())
    }

    fn decode(buf: &[u8], frames: &mut Vec<FrameSetPacket>, owned: bool) -> Result<()> {
        frames.clear();
        let mut reader = RaknetReader::new(buf);
        let id = reader.read_u8()?;
        if !(0x80..=0x8d).contains(&id) {
            return Err(RaknetError::PacketHeaderError);
        }
        let sequence_number = reader.read_u24(Endian::Little)?;
        if reader.pos() == buf.len() as u64 {
            return Err(RaknetError::PacketParseError);
        }
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
                data: FramePayload::default(),
                shared_payload: None,
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
            // Multi-frame datagrams retain independent payload allocations.
            // The owned single-frame path moves the original allocation below.
            if !owned || !frames.is_empty() || reader.pos() != buf.len() as u64 {
                frame.data = data.to_vec().into();
            }
            frames.push(frame);
        }

        Ok(())
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

    #[cfg(test)]
    pub fn get_ack(&mut self) -> Vec<(u32, u32)> {
        std::mem::take(&mut self.ack)
    }

    fn take_ack_into(&mut self, output: &mut Vec<(u32, u32)>) {
        output.clear();
        std::mem::swap(output, &mut self.ack);
    }

    fn take_nack_into(&mut self, output: &mut Vec<(u32, u32)>) {
        output.clear();
        std::mem::swap(output, &mut self.nack);
    }

    #[cfg(test)]
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

    #[cfg(test)]
    pub fn get_ack(&mut self) -> Vec<(u32, u32)> {
        self.sequence_number_ackset.get_ack()
    }

    #[cfg(test)]
    pub fn get_nack(&mut self) -> Vec<(u32, u32)> {
        self.sequence_number_ackset.get_nack()
    }

    pub(crate) fn take_ack_into(&mut self, output: &mut Vec<(u32, u32)>) {
        self.sequence_number_ackset.take_ack_into(output);
    }

    pub(crate) fn take_nack_into(&mut self, output: &mut Vec<(u32, u32)>) {
        self.sequence_number_ackset.take_nack_into(output);
    }

    #[cfg(test)]
    pub fn flush(&mut self, _peer_addr: &SocketAddr) -> Vec<FrameSetPacket> {
        let mut frames = Vec::new();
        self.flush_into(&mut frames);
        frames
    }

    pub(crate) fn flush_into(&mut self, frames: &mut Vec<FrameSetPacket>) {
        frames.append(&mut self.ready_ordered);
        self.ready_bytes = 0;
        if self.packets.is_empty() {
            return;
        }
        let mut packets: Vec<_> = self.packets.drain().map(|(_, frame)| frame).collect();
        packets.sort_unstable_by_key(|frame| frame.sequence_number);
        frames.extend(packets);
    }
    pub(crate) fn needs_maintenance(&self) -> bool {
        !self.sequence_number_ackset.nack.is_empty() || self.fragment_queue.size() != 0
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

#[cfg(feature = "send-policy")]
struct SendScheduler {
    capacity_notifier: std::sync::Weak<tokio::sync::Notify>,
    options: crate::SendOptions,
    unreliable: VecDeque<FrameSetPacket>,
    unreliable_bytes: usize,
    use_legacy: bool,
    persistent: bool,
    next_lane: usize,
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
    flight_order_dirty: bool,
    next_retry: i64,
    retries_pending: bool,
    coalesce: bool,
    coalescing_allowed: bool,
    grouped_acks: bool,
    #[cfg(feature = "send-policy")]
    scheduler: Option<Box<SendScheduler>>,
}

impl SendQ {
    pub const DEFAULT_TIMEOUT_MILLS: i64 = 50;
    const MAX_IN_FLIGHT_PACKETS: usize = 64;
    const MAX_COALESCED_FRAMES: usize = 8;
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
            #[cfg(feature = "send-policy")]
            scheduler: None,
            queued_unreliable: 0,
            buffered_bytes: 0,
            sent_packet: vec![],
            flight_order_dirty: false,
            next_retry: i64::MAX,
            retries_pending: false,
            coalesce: false,
            coalescing_allowed: true,
            grouped_acks: false,
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
        #[cfg(feature = "send-policy")]
        {
            self.has_reserved_capacity(reliability, required)
        }
        #[cfg(not(feature = "send-policy"))]
        {
            Ok(self.buffered_bytes.saturating_add(required) <= Self::SEND_HIGH_WATER.max(required))
        }
    }

    pub(crate) fn enable_coalescing(&mut self) {
        self.coalesce = self.coalescing_allowed;
    }

    pub(crate) fn allows_coalescing(&self) -> bool {
        self.coalescing_allowed
    }

    pub(crate) fn has_batch_capacity(
        &self,
        reliability: Reliability,
        mut lengths: impl Iterator<Item = usize>,
    ) -> Result<bool> {
        let required = lengths.try_fold(0usize, |total, length| {
            total
                .checked_add(self.required_bytes(reliability, length)?)
                .filter(|&bytes| bytes <= Self::MAX_BUFFERED_BYTES)
                .ok_or(RaknetError::PacketSizeExceedMTU)
        })?;
        #[cfg(feature = "send-policy")]
        {
            self.has_batch_required_capacity(reliability, required)
        }
        #[cfg(not(feature = "send-policy"))]
        {
            Ok(self.buffered_bytes.saturating_add(required) <= Self::SEND_HIGH_WATER.max(required))
        }
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
        self.insert_payload(reliability, buf, order_channel, None)
    }

    pub(crate) fn insert_bytes(
        &mut self,
        reliability: Reliability,
        data: &bytes::Bytes,
        order_channel: u8,
    ) -> Result<()> {
        self.insert_payload(reliability, data, order_channel, Some(data))
    }

    fn insert_payload(
        &mut self,
        reliability: Reliability,
        buf: &[u8],
        order_channel: u8,
        owned: Option<&bytes::Bytes>,
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

                let frame = FrameSetPacket::from_payload(
                    reliability,
                    owned.map_or_else(
                        || FramePayload::Owned(buf.to_vec()),
                        |data| FramePayload::Bytes(data.clone()),
                    ),
                );
                #[cfg(feature = "send-policy")]
                self.push_unreliable(frame);
                #[cfg(not(feature = "send-policy"))]
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
                let mut frame = FrameSetPacket::from_payload(
                    reliability,
                    owned.map_or_else(
                        || FramePayload::Owned(buf.to_vec()),
                        |data| FramePayload::Bytes(data.clone()),
                    ),
                );
                frame.order_channel = order_channel;
                frame.ordered_frame_index = ordered_frame_index;
                frame.sequenced_frame_index = sequenced_frame_index;
                #[cfg(feature = "send-policy")]
                self.push_unreliable(frame);
                #[cfg(not(feature = "send-policy"))]
                self.packets.push_back(frame);
            }
            Reliability::Reliable => {
                if buf.len() > max_payload {
                    return Err(RaknetError::PacketSizeExceedMTU);
                }

                let mut frame = FrameSetPacket::from_payload(
                    reliability,
                    owned.map_or_else(
                        || FramePayload::copy_shared(buf),
                        |data| FramePayload::Bytes(data.clone()),
                    ),
                );
                frame.reliable_frame_index = self.reliable_frame_index;
                self.packets.push_back(frame);
                self.reliable_frame_index = sequence::next(self.reliable_frame_index);
            }
            Reliability::ReliableOrdered => {
                if buf.len() <= max_payload {
                    let mut frame = FrameSetPacket::from_payload(
                        reliability,
                        owned.map_or_else(
                            || FramePayload::copy_shared(buf),
                            |data| FramePayload::Bytes(data.clone()),
                        ),
                    );
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

                    let data: Option<Arc<[u8]>> = owned.is_none().then(|| Arc::from(buf));
                    let shared_payload = Arc::new(SharedPayload {
                        remaining: AtomicUsize::new(compound_size as usize),
                        bytes: buf.len(),
                    });
                    for (fragment_index, chunk) in buf.chunks(max_payload).enumerate() {
                        let start = fragment_index * max_payload;
                        let mut frame = FrameSetPacket::from_payload(
                            reliability,
                            match owned {
                                Some(data) => {
                                    FramePayload::Bytes(data.slice(start..start + chunk.len()))
                                }
                                None => FramePayload::Shared {
                                    owner: data.as_ref().unwrap().clone(),
                                    start: start as u32,
                                    length: chunk.len() as u16,
                                },
                            },
                        );
                        frame.shared_payload = Some(shared_payload.clone());
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
                let mut frame = FrameSetPacket::from_payload(
                    reliability,
                    owned.map_or_else(
                        || FramePayload::copy_shared(buf),
                        |data| FramePayload::Bytes(data.clone()),
                    ),
                );
                frame.order_channel = order_channel;
                frame.reliable_frame_index = self.reliable_frame_index;
                frame.sequenced_frame_index = sequenced_frame_index;
                frame.ordered_frame_index = ordered_frame_index;
                self.packets.push_back(frame);
                self.reliable_frame_index = sequence::next(self.reliable_frame_index);
            }
        };
        self.buffered_bytes += reserved;
        #[cfg(not(feature = "send-policy"))]
        if matches!(
            reliability,
            Reliability::Unreliable | Reliability::UnreliableSequenced
        ) {
            self.queued_unreliable += 1;
        }
        Ok(())
    }

    fn update_rto(&mut self, rtt: i64) {
        let previous = self.rto;
        (self.srtt, self.rto) = Self::latency_sample(self.srtt, rtt);
        if self.rto != previous {
            self.next_retry = 0;
        }
    }

    fn latency_sample(srtt: i64, rtt: i64) -> (i64, i64) {
        let srtt = (srtt * 4 + rtt) / 5;
        (
            srtt,
            (srtt * 3 / 2).clamp(Self::RTO_LBOUND, Self::RTO_UBOUND),
        )
    }

    pub(crate) fn mtu(&self) -> u16 {
        self.mtu
    }

    pub fn get_rto(&self) -> i64 {
        self.rto
    }

    pub fn nack(&mut self, sequence: u32, tick: i64) {
        for i in 0..self.sent_packet.len() {
            let item = &mut self.sent_packet[i];
            if item.1 && item.0.sequence_number == sequence {
                // Preserve ordinary pacing once this connection has observed
                // loss, including feedback received before batching is enabled.
                self.coalescing_allowed = false;
                self.coalesce = false;
                raknet_log_debug!(
                    "packet {}-{}-{} nack {} times",
                    item.0.sequence_number,
                    item.0.reliable_frame_index,
                    item.0.ordered_frame_index,
                    item.3 + 1
                );
                let previous = item.0.sequence_number;
                self.flight_order_dirty = true;
                item.0.sequence_number = self.sequence_number;
                self.sequence_number = sequence::next(self.sequence_number);
                item.1 = false;
                self.retries_pending = true;
                item.2 = tick;

                if item.4.len() == 64 {
                    item.4.remove(0);
                }
                item.4.push(previous);
            }
        }
    }

    pub fn ack(&mut self, sequence: u32, tick: i64) {
        if self.grouped_acks && self.sent_packet.len() > 1 {
            self.ack_ranges(&[(sequence, sequence)], tick);
            return;
        }
        // Ignore ACKs outside the actual send history before advancing any state.
        let Some(index) = self
            .sent_packet
            .iter()
            .position(|item| item.0.sequence_number == sequence || item.4.contains(&sequence))
        else {
            return;
        };
        let item = self.sent_packet.remove(index);
        if self.grouped_acks && self.sent_packet.is_empty() {
            self.grouped_acks = false;
        }
        let released_payload = match &item.0.shared_payload {
            Some(payload) if payload.remaining.fetch_sub(1, Ordering::Relaxed) == 1 => {
                payload.bytes
            }
            Some(_) => 0,
            None => item.0.data.len(),
        };
        self.buffered_bytes -= released_payload + Self::FRAME_BUDGET;
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

    #[inline]
    pub fn ack_ranges(&mut self, ranges: &[(u32, u32)], tick: i64) {
        if self.grouped_acks {
            self.ack_ranges_inner::<true>(ranges, tick);
        } else {
            self.ack_ranges_inner::<false>(ranges, tick);
        }
    }

    fn ack_ranges_inner<const GROUPED: bool>(&mut self, ranges: &[(u32, u32)], tick: i64) {
        if let [(start, end)] = ranges {
            if start == end && (!GROUPED || self.sent_packet.len() <= 1) {
                self.ack(*start, tick);
                return;
            }
        }
        // Compact the bounded flight window once. Removing each acknowledged
        // frame separately repeatedly shifts the remaining frame metadata.
        let previous = self.ack_sequence_number;
        let mut newest = previous;
        let mut latency = (self.srtt, self.rto);
        let mut released_bytes = 0;
        let mut previous_sample = None;
        self.sent_packet.retain_mut(|item| {
            let acknowledged_id = std::iter::once(item.0.sequence_number)
                .chain(item.4.iter().copied())
                .find(|id| {
                    ranges
                        .iter()
                        .any(|&(start, end)| start <= *id && *id <= end)
                });
            let Some(id) = acknowledged_id else {
                return true;
            };
            let payload_bytes = match &item.0.shared_payload {
                Some(payload) if payload.remaining.fetch_sub(1, Ordering::Relaxed) == 1 => {
                    payload.bytes
                }
                Some(_) => 0,
                None => item.0.data.len(),
            };
            released_bytes += payload_bytes + Self::FRAME_BUDGET;
            if sequence::newer(id, newest) {
                newest = id;
            }
            // Initial members of a frame set are contiguous. Retransmitted
            // frames never contribute RTT samples (Karn's algorithm).
            if item.3 == 0 && (!GROUPED || previous_sample != Some(id)) {
                if GROUPED {
                    previous_sample = Some(id);
                }
                latency = Self::latency_sample(latency.0, tick.saturating_sub(item.2).max(0));
            }
            false
        });
        self.buffered_bytes -= released_bytes;
        if GROUPED && self.sent_packet.is_empty() {
            self.grouped_acks = false;
        }

        if self.rto != latency.1 {
            self.next_retry = 0;
        }
        (self.srtt, self.rto) = latency;
        if newest != previous {
            let span = sequence::distance(newest, previous);
            let missing: Vec<_> = self
                .sent_packet
                .iter()
                .filter_map(|item| {
                    let distance = sequence::distance(item.0.sequence_number, previous);
                    (distance > 0 && distance < span).then_some(item.0.sequence_number)
                })
                .collect();
            for id in missing {
                self.nack(id, tick);
            }
            self.ack_sequence_number = newest;
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

    fn retry_timeout(rto: i64, retries: u32) -> i64 {
        let mut timeout = rto;
        for _ in 0..retries.min(32) {
            timeout = ((timeout as f64 * 1.5) as i64).min(Self::RTO_UBOUND);
        }
        timeout
    }

    fn tick(&mut self, tick: i64) {
        if tick < self.next_retry {
            return;
        }
        self.next_retry = i64::MAX;
        for i in 0..self.sent_packet.len() {
            let p = &mut self.sent_packet[i];

            let cur_rto = Self::retry_timeout(self.rto, p.3);

            if p.1 && tick - p.2 >= cur_rto {
                self.coalescing_allowed = false;
                self.coalesce = false;
                let previous = p.0.sequence_number;
                self.flight_order_dirty = true;
                p.0.sequence_number = self.sequence_number;
                self.sequence_number = sequence::next(self.sequence_number);
                p.1 = false;
                self.retries_pending = true;
                if p.4.len() == 64 {
                    p.4.remove(0);
                }
                p.4.push(previous);
            } else if p.1 {
                self.next_retry = self.next_retry.min(p.2.saturating_add(cur_rto));
            }
        }
    }

    pub fn flush(&mut self, tick: i64, peer_addr: &SocketAddr) -> OutgoingFrames {
        #[cfg(feature = "send-policy")]
        if self
            .scheduler
            .as_ref()
            .is_some_and(|s| !s.use_legacy || !s.unreliable.is_empty())
        {
            return self.flush_scheduled(tick);
        }
        self.tick(tick);

        // Reserve queued batches once; a full reliable flight window reserves
        // nothing. The compact vector keeps async frame transfers cheap.
        let capacity = self.packets.len().min(
            Self::MAX_IN_FLIGHT_PACKETS.saturating_sub(self.sent_packet.len())
                + self.queued_unreliable,
        );
        let mut ret = OutgoingFrames::with_capacity(capacity);
        // New sends preserve numeric order until the 24-bit sequence wraps.
        // ACK removal preserves it too; only resequencing requires sorting.
        if self.flight_order_dirty {
            self.sent_packet
                .sort_unstable_by_key(|packet| packet.0.sequence_number);
            self.flight_order_dirty = false;
        }

        if self.retries_pending {
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
                    self.next_retry = self
                        .next_retry
                        .min(tick.saturating_add(Self::retry_timeout(self.rto, packet.3)));
                }
            }
            self.retries_pending = false;
        }

        let mut group_sequence = None;
        let mut group_bytes = 0;
        let mut group_frames = 0;
        let datagram_budget = usize::from(self.mtu).saturating_sub(28);
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

            if self.coalesce {
                let packable = matches!(packet.reliability(), Ok(Reliability::ReliableOrdered))
                    && !packet.is_fragment();
                let frame_bytes = packet._size().expect("queued frames were validated") - 4;
                if packable
                    && group_sequence.is_some()
                    && group_frames < Self::MAX_COALESCED_FRAMES
                    && group_bytes + frame_bytes <= datagram_budget
                {
                    packet.sequence_number = group_sequence.unwrap();
                    group_bytes += frame_bytes;
                    group_frames += 1;
                    self.grouped_acks = true;
                    ret.coalesced = true;
                } else {
                    packet.sequence_number = self.sequence_number;
                    self.sequence_number = sequence::next(self.sequence_number);
                    group_sequence = packable.then_some(packet.sequence_number);
                    group_bytes = 4 + frame_bytes;
                    // Keep a 16-message application window spread across at
                    // least two datagrams so later packets can expose a loss.
                    group_frames = 1;
                }
            } else {
                packet.sequence_number = self.sequence_number;
                self.sequence_number = sequence::next(self.sequence_number);
            }

            if reliable {
                ret.push(packet.clone());
                if self
                    .sent_packet
                    .last()
                    .is_some_and(|previous| previous.0.sequence_number > packet.sequence_number)
                {
                    self.flight_order_dirty = true;
                }
                self.sent_packet.push((packet, true, tick, 0, Vec::new()));
                self.next_retry = self.next_retry.min(tick.saturating_add(self.rto));
            } else {
                self.queued_unreliable -= 1;
                self.buffered_bytes -= packet.data.len() + Self::FRAME_BUDGET;
                ret.push(packet);
            }
        }

        ret
    }

    pub fn is_empty(&self) -> bool {
        #[cfg(feature = "send-policy")]
        {
            self.packets.is_empty()
                && self.sent_packet.is_empty()
                && self
                    .scheduler
                    .as_ref()
                    .is_none_or(|s| s.unreliable.is_empty())
        }
        #[cfg(not(feature = "send-policy"))]
        {
            self.packets.is_empty() && self.sent_packet.is_empty()
        }
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

#[cfg(feature = "send-policy")]
impl SendQ {
    /// Validate batch sizes and determine packing eligibility in one pass.
    /// Oversized messages still validate their fragmentation budget, but cannot
    /// be packed. Avoid rescanning an entire batch that will use ordinary sends.
    pub(crate) fn batch_can_coalesce(
        &self,
        reliability: Reliability,
        mut lengths: impl Iterator<Item = usize>,
    ) -> Result<bool> {
        let allowed = self.allows_coalescing() && reliability == Reliability::ReliableOrdered;
        if reliability == Reliability::ReliableOrdered && !allowed {
            self.has_batch_capacity(reliability, lengths)?;
            return Ok(false);
        }
        let max_payload = usize::from(self.mtu).saturating_sub(60);
        let datagram_budget = usize::from(self.mtu).saturating_sub(28);
        let mut previous = None;
        let mut packable = false;
        let required = lengths.try_fold(0usize, |total, length| {
            if reliability != Reliability::ReliableOrdered && length > max_payload {
                return Err(RaknetError::PacketSizeExceedMTU);
            }
            if allowed && !packable && length <= max_payload {
                packable = previous.is_some_and(|last: usize| {
                    last <= max_payload
                        && last.saturating_add(length).saturating_add(24) <= datagram_budget
                });
            }
            previous = Some(length);
            total
                .checked_add(self.required_bytes(reliability, length)?)
                .filter(|&bytes| bytes <= Self::MAX_BUFFERED_BYTES)
                .ok_or(RaknetError::PacketSizeExceedMTU)
        })?;
        self.has_batch_required_capacity(reliability, required)?;
        Ok(allowed && packable)
    }

    fn has_batch_required_capacity(
        &self,
        reliability: Reliability,
        required: usize,
    ) -> Result<bool> {
        // Unreliable batches use the streaming path. Their aggregate may
        // exceed the pending reserve even though every member fits and drains.
        if matches!(
            reliability,
            Reliability::Unreliable | Reliability::UnreliableSequenced
        ) && self
            .send_options()
            .is_some_and(|options| required > options.unreliable_queue_bytes)
        {
            return Ok(false);
        }
        self.has_reserved_capacity(reliability, required)
    }

    fn ensure_scheduler(&mut self) -> &mut SendScheduler {
        if self.scheduler.is_none() {
            let mut unreliable = VecDeque::with_capacity(self.queued_unreliable);
            let mut unreliable_bytes = 0;
            for _ in 0..self.packets.len() {
                let frame = self.packets.pop_front().unwrap();
                if frame.is_reliable().unwrap_or(false) {
                    self.packets.push_back(frame);
                } else {
                    unreliable_bytes += frame.data.len() + Self::FRAME_BUDGET;
                    unreliable.push_back(frame);
                }
            }
            self.queued_unreliable = 0;
            self.scheduler = Some(Box::new(SendScheduler {
                capacity_notifier: std::sync::Weak::new(),
                options: crate::SendOptions::default(),
                unreliable,
                unreliable_bytes,
                use_legacy: usize::from(self.mtu) * Self::MAX_IN_FLIGHT_PACKETS
                    <= crate::SendOptions::default().flush_bytes,
                persistent: true,
                next_lane: 2,
            }));
        }
        self.scheduler.as_mut().unwrap()
    }

    fn push_unreliable(&mut self, mut frame: FrameSetPacket) {
        if let Some(scheduler) = &mut self.scheduler {
            scheduler.unreliable_bytes += frame.data.len() + Self::FRAME_BUDGET;
            scheduler.unreliable.push_back(frame);
        } else {
            frame.sequence_number = self.sequence_number;
            self.sequence_number = sequence::next(self.sequence_number);
            self.queued_unreliable += 1;
            self.packets.push_back(frame);
        }
    }

    /// Bind queue-release notifications only after a policy is enabled.
    pub(crate) fn register_capacity_notifier(&mut self, notifier: &Arc<tokio::sync::Notify>) {
        let Some(scheduler) = &mut self.scheduler else {
            return;
        };
        if !std::ptr::eq(scheduler.capacity_notifier.as_ptr(), Arc::as_ptr(notifier)) {
            scheduler.capacity_notifier = Arc::downgrade(notifier);
        }
    }

    pub(crate) fn set_send_options(&mut self, options: crate::SendOptions) -> Result<()> {
        let mtu = usize::from(self.mtu);
        if options.in_flight_bytes < mtu
            || options.flush_bytes < mtu
            || options.unreliable_queue_bytes < mtu
        {
            return Err(RaknetError::PacketSizeExceedMTU);
        }
        let scheduler = self.ensure_scheduler();
        scheduler.options = options;
        scheduler.persistent = true;
        let defaults = crate::SendOptions::default();
        scheduler.use_legacy = options.in_flight_frames == defaults.in_flight_frames
            && options.in_flight_bytes == defaults.in_flight_bytes
            && options.flush_bytes == defaults.flush_bytes
            && mtu * Self::MAX_IN_FLIGHT_PACKETS <= options.flush_bytes;
        Ok(())
    }

    pub(crate) fn send_options(&self) -> Option<crate::SendOptions> {
        self.scheduler.as_ref().map(|s| s.options)
    }

    #[inline]
    fn has_reserved_capacity(&self, reliability: Reliability, required: usize) -> Result<bool> {
        if self.scheduler.is_none() {
            return Ok(
                self.buffered_bytes.saturating_add(required) <= Self::SEND_HIGH_WATER.max(required)
            );
        }
        let unreliable = matches!(
            reliability,
            Reliability::Unreliable | Reliability::UnreliableSequenced
        );
        if !unreliable {
            // The common reliable-only case needs neither the full options
            // copy nor reservation arithmetic. Valid budgets leave room for
            // the unreliable reserve, including when no unreliable data waits.
            let budget = match &self.scheduler {
                None if required <= Self::MAX_BUFFERED_BYTES - 64 * 1024 => {
                    Some(Self::SEND_HIGH_WATER.max(required))
                }
                None => None,
                Some(s)
                    if s.unreliable_bytes == 0 && required <= s.options.reliable_queue_bytes =>
                {
                    Some(s.options.reliable_queue_bytes)
                }
                Some(_) => None,
            };
            if let Some(budget) = budget {
                return Ok(self.buffered_bytes.saturating_add(required) <= budget);
            }
        }
        let options = self.send_options().unwrap();
        let used_unreliable = self.scheduler.as_ref().map_or(0, |s| s.unreliable_bytes);
        let (used, limit) = if unreliable {
            if required > options.unreliable_queue_bytes {
                return Err(RaknetError::PacketSizeExceedMTU);
            }
            (used_unreliable, options.unreliable_queue_bytes)
        } else {
            let hard_limit = Self::MAX_BUFFERED_BYTES - options.unreliable_queue_bytes;
            if required > hard_limit {
                return Err(RaknetError::PacketSizeExceedMTU);
            }
            (
                self.buffered_bytes - used_unreliable,
                options.reliable_queue_bytes.max(required).min(hard_limit),
            )
        };
        Ok(used.saturating_add(required) <= limit
            && self.buffered_bytes.saturating_add(required) <= Self::MAX_BUFFERED_BYTES)
    }

    fn flush_scheduled(&mut self, tick: i64) -> OutgoingFrames {
        self.tick(tick);
        if self.flight_order_dirty {
            self.sent_packet
                .sort_unstable_by_key(|packet| packet.0.sequence_number);
            self.flight_order_dirty = false;
        }
        // Taking the sidecar keeps the frame queues and connection indexes under
        // one mutable owner. Unconfigured sockets never allocate this state.
        let mut scheduler = self.scheduler.take().unwrap();
        let options = scheduler.options;
        // Compute only in configured scheduling. ACK handling remains the
        // original path with no per-frame policy or byte-ledger checks.
        let mut flight_bytes: usize = self.sent_packet.iter().map(|p| p.0._size().unwrap()).sum();
        let capacity = self.packets.len().min(
            options
                .in_flight_frames
                .saturating_sub(self.sent_packet.len()),
        ) + scheduler.unreliable.len();
        let mut output = OutgoingFrames::with_capacity(capacity.min(256));
        let quantum = usize::from(self.mtu) * 4;
        let mut bytes = 0;
        let mut unreliable_drained = false;
        let mut retry_cursor = 0;
        let mut group_sequence = None;
        let mut group_bytes = 0;
        let mut group_frames = 0;
        let datagram_budget = usize::from(self.mtu).saturating_sub(28);
        'rounds: loop {
            let mut progressed = false;
            let start = scheduler.next_lane;
            for offset in 0..3 {
                let lane = (start + offset) % 3;
                let mut lane_bytes = 0;
                while lane_bytes < quantum {
                    let charge = match lane {
                        0 => scheduler
                            .unreliable
                            .front()
                            .map(|frame| frame._size().unwrap()),
                        1 => {
                            if !self.retries_pending {
                                break;
                            }
                            while retry_cursor < self.sent_packet.len()
                                && self.sent_packet[retry_cursor].1
                            {
                                retry_cursor += 1;
                            }
                            self.sent_packet
                                .get(retry_cursor)
                                .map(|item| item.0._size().unwrap())
                        }
                        _ => self.packets.front().and_then(|frame| {
                            let size = frame._size().unwrap();
                            (self.sent_packet.len() < options.in_flight_frames
                                && flight_bytes + size <= options.in_flight_bytes)
                                .then_some(size)
                        }),
                    };
                    let Some(charge) = charge else {
                        break;
                    };
                    // Skip a class that cannot fit the remaining burst; smaller
                    // realtime messages in another class can still make progress.
                    if bytes + charge > options.flush_bytes {
                        break;
                    }
                    match lane {
                        0 => {
                            let mut packet = scheduler.unreliable.pop_front().unwrap();
                            packet.sequence_number = self.sequence_number;
                            self.sequence_number = sequence::next(self.sequence_number);
                            let reserved = packet.data.len() + Self::FRAME_BUDGET;
                            self.buffered_bytes -= reserved;
                            scheduler.unreliable_bytes -= reserved;
                            unreliable_drained = true;
                            output.push(packet);
                            group_sequence = None;
                        }
                        1 => {
                            let item = &mut self.sent_packet[retry_cursor];
                            output.push(item.0.clone());
                            item.1 = true;
                            item.2 = tick;
                            item.3 = item.3.saturating_add(1);
                            self.next_retry = self
                                .next_retry
                                .min(tick.saturating_add(Self::retry_timeout(self.rto, item.3)));
                            retry_cursor += 1;
                            group_sequence = None;
                        }
                        _ => {
                            let mut packet = self.packets.pop_front().unwrap();
                            let packable = self.coalesce
                                && matches!(packet.reliability(), Ok(Reliability::ReliableOrdered))
                                && !packet.is_fragment();
                            let frame_bytes = charge - 4;
                            if packable
                                && group_sequence.is_some()
                                && group_frames < Self::MAX_COALESCED_FRAMES
                                && group_bytes + frame_bytes <= datagram_budget
                            {
                                packet.sequence_number = group_sequence.unwrap();
                                group_bytes += frame_bytes;
                                group_frames += 1;
                                self.grouped_acks = true;
                                output.coalesced = true;
                            } else {
                                packet.sequence_number = self.sequence_number;
                                self.sequence_number = sequence::next(self.sequence_number);
                                group_sequence = packable.then_some(packet.sequence_number);
                                group_bytes = charge;
                                group_frames = 1;
                            }
                            output.push(packet.clone());
                            if self
                                .sent_packet
                                .last()
                                .is_some_and(|p| p.0.sequence_number > packet.sequence_number)
                            {
                                self.flight_order_dirty = true;
                            }
                            self.sent_packet.push((packet, true, tick, 0, Vec::new()));
                            flight_bytes += charge;
                            self.next_retry = self.next_retry.min(tick.saturating_add(self.rto));
                        }
                    }
                    progressed = true;
                    bytes += charge;
                    lane_bytes += charge;
                    scheduler.next_lane = (lane + 1) % 3;
                    if bytes == options.flush_bytes {
                        break 'rounds;
                    }
                }
            }
            if !progressed {
                break;
            }
        }
        self.retries_pending = self.sent_packet.iter().any(|item| !item.1);
        if unreliable_drained {
            if let Some(capacity) = scheduler.capacity_notifier.upgrade() {
                capacity.notify_waiters();
            }
        }
        if scheduler.persistent || !scheduler.use_legacy || !scheduler.unreliable.is_empty() {
            self.scheduler = Some(scheduler);
        }
        output
    }
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
    assert_eq!(ret[0].data.as_ref(), &[1, 2, 3]);
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

#[cfg(test)]
mod allocation_regressions {
    use super::*;

    #[test]
    fn frame_decoder_reuses_storage_and_discards_previous_frames() {
        let packet = FrameSetPacket::new(Reliability::ReliableOrdered, vec![0xfe; 800]);
        let encoded = packet.serialize().unwrap();
        let mut frames = Vec::new();
        FrameVec::decode_into(&encoded, &mut frames).unwrap();
        let capacity = frames.capacity();
        assert_eq!(frames[0].data.as_ref(), &[0xfe; 800]);
        frames.clear();
        FrameVec::decode_into(&encoded, &mut frames).unwrap();
        assert_eq!(frames.len(), 1);
        assert_eq!(frames.capacity(), capacity);
        assert!(FrameVec::decode_into(&[0x84], &mut frames).is_err());
        assert!(frames.is_empty());
    }

    #[test]
    fn ordered_delivery_retains_the_receive_queue_allocation() {
        let mut queue = RecvQ::new();
        let mut delivered = Vec::new();
        for index in 0..16 {
            let mut frame = FrameSetPacket::new(Reliability::ReliableOrdered, vec![0xfe]);
            frame.sequence_number = index;
            frame.reliable_frame_index = index;
            frame.ordered_frame_index = index;
            queue.insert(frame).unwrap();
            queue.flush_into(&mut delivered);
            assert_eq!(delivered.len(), 1);
            delivered.clear();
            assert!(queue.ready_ordered.capacity() > 0);
        }
    }
}

#[cfg(test)]
mod serialization_reuse_tests {
    use super::*;

    #[test]
    fn reused_fragment_buffer_preserves_the_wire_format() {
        let mut frame = FrameSetPacket::new(Reliability::ReliableOrdered, vec![0xfe; 16]);
        frame.flags |= 16;
        frame.sequence_number = 0xabcdef;
        frame.reliable_frame_index = 0x123456;
        frame.ordered_frame_index = 0x654321;
        frame.order_channel = 7;
        frame.compound_size = 3;
        frame.compound_id = 0x1234;
        frame.fragment_index = 1;
        let mut expected = vec![
            0x8c, 0xef, 0xcd, 0xab, 0x70, 0, 0x80, 0x56, 0x34, 0x12, 0x21, 0x43, 0x65, 7, 0, 0, 0,
            3, 0x12, 0x34, 0, 0, 0, 1,
        ];
        expected.extend_from_slice(&[0xfe; 16]);
        let mut buffer = Vec::with_capacity(128);
        buffer.extend_from_slice(&[0xff; 64]);
        for _ in 0..100 {
            frame.serialize_into(&mut buffer).unwrap();
            assert_eq!(buffer, expected);
            assert_eq!(buffer.capacity(), 128);
        }
        frame.compound_size = 0;
        assert!(frame.serialize_into(&mut buffer).is_err());
        assert_eq!(buffer, expected);
    }
}

#[cfg(test)]
mod shared_fragment_tests {
    use super::*;

    #[test]
    fn retransmitted_fragment_accepts_old_ack_without_releasing_the_budget_twice() {
        let peer = "127.0.0.1:19132".parse().unwrap();
        let mut queue = SendQ::new(1400);
        queue
            .insert(Reliability::ReliableOrdered, &[0xfe; 4096])
            .unwrap();
        let frames = queue.flush(0, &peer);
        let old_id = frames[3].sequence_number;
        queue.nack(old_id, 1);
        let retry = queue.flush(1, &peer);
        assert_eq!(retry.len(), 1);
        let new_id = retry[0].sequence_number;
        queue.ack(old_id, 2);
        let budget = queue.buffered_bytes;
        queue.ack(old_id, 2);
        queue.ack(new_id, 2);
        assert_eq!(queue.buffered_bytes, budget);
        for frame in &frames[..3] {
            queue.ack(frame.sequence_number, 2);
        }
        assert_eq!(queue.buffered_bytes, 0);
    }

    #[test]
    fn fragment_slices_share_one_allocation_and_keep_its_budget_until_final_ack() {
        let peer = "127.0.0.1:19132".parse().unwrap();
        let mut queue = SendQ::new(1400);
        let payload = vec![0xfe; 4096];
        queue
            .insert(Reliability::ReliableOrdered, &payload)
            .unwrap();
        let frames = queue.flush(0, &peer);
        assert_eq!(frames.len(), 4);
        let owner = frames[0].shared_payload.as_ref().unwrap();
        for frame in &frames {
            assert!(Arc::ptr_eq(owner, frame.shared_payload.as_ref().unwrap()));
        }
        let restored: Vec<_> = frames
            .iter()
            .flat_map(|frame| frame.data.iter().copied())
            .collect();
        assert_eq!(restored, payload);
        let initial = queue.buffered_bytes;
        for (index, frame) in frames.iter().enumerate() {
            queue.ack(frame.sequence_number, 1);
            queue.ack(frame.sequence_number, 1);
            if index + 1 < frames.len() {
                assert_eq!(
                    queue.buffered_bytes,
                    initial - (index + 1) * SendQ::FRAME_BUDGET
                );
            }
        }
        assert_eq!(queue.buffered_bytes, 0);
        assert_eq!(owner.remaining.load(Ordering::Relaxed), 0);
    }
}

#[cfg(test)]
mod flight_order_tests {
    use super::*;

    #[test]
    fn retries_and_sequence_wrap_keep_the_existing_numeric_order() {
        let peer = "127.0.0.1:19132".parse().unwrap();
        let mut queue = SendQ::new(1400);
        queue.sequence_number = sequence::MASK;
        queue.insert(Reliability::ReliableOrdered, &[0xfe]).unwrap();
        queue.insert(Reliability::ReliableOrdered, &[0xfe]).unwrap();
        let initial = queue.flush(0, &peer);
        assert_eq!(
            initial
                .iter()
                .map(|frame| frame.sequence_number)
                .collect::<Vec<_>>(),
            [sequence::MASK, 0]
        );
        assert!(queue.flight_order_dirty);
        assert!(queue.flush(1, &peer).is_empty());
        assert_eq!(
            queue
                .sent_packet
                .iter()
                .map(|packet| packet.0.sequence_number)
                .collect::<Vec<_>>(),
            [0, sequence::MASK]
        );
        assert!(!queue.flight_order_dirty);
        queue.nack(sequence::MASK, 2);
        assert!(queue.flight_order_dirty);
        let retry = queue.flush(2, &peer);
        assert_eq!(retry.len(), 1);
        assert_eq!(retry[0].sequence_number, 1);
        assert!(!queue.flight_order_dirty);
        assert_eq!(
            queue
                .sent_packet
                .iter()
                .map(|packet| packet.0.sequence_number)
                .collect::<Vec<_>>(),
            [0, 1]
        );
        queue.ack(sequence::MASK, 3);
        queue.ack(0, 3);
        assert!(queue.is_empty());
        assert_eq!(queue.buffered_bytes, 0);
    }
}

#[cfg(test)]
mod owned_decode_tests {
    use super::*;

    #[test]
    fn a_single_frame_moves_the_udp_allocation_into_its_payload() {
        let frame = FrameSetPacket::new(Reliability::ReliableOrdered, vec![0xfe; 800]);
        let packet = frame.serialize().unwrap();
        let pointer = packet.as_ptr();
        let mut decoded = Vec::new();
        FrameVec::decode_owned_into(packet, &mut decoded).unwrap();
        assert_eq!(decoded.len(), 1);
        assert_eq!(decoded[0].data.as_ptr(), pointer);
        assert_eq!(decoded[0].data.as_ref(), frame.data.as_ref());
        let data: Vec<u8> = decoded.pop().unwrap().data.into();
        assert_eq!(data.as_ptr(), pointer);
    }

    #[test]
    fn multiple_frames_and_invalid_lengths_keep_the_borrowed_decoder_contract() {
        let first = FrameSetPacket::new(Reliability::ReliableOrdered, vec![0xfe; 800]);
        let second = FrameSetPacket::new(Reliability::Unreliable, vec![0xfe; 64]);
        let mut packet = first.serialize().unwrap();
        packet.extend_from_slice(&second.serialize().unwrap()[4..]);
        let mut expected = Vec::new();
        FrameVec::decode_into(&packet, &mut expected).unwrap();
        let mut actual = Vec::new();
        FrameVec::decode_owned_into(packet.clone(), &mut actual).unwrap();
        assert_eq!(actual.len(), 2);
        for (actual, expected) in actual.iter().zip(&expected) {
            assert_eq!(actual.serialize().unwrap(), expected.serialize().unwrap());
        }
        for length in 0..packet.len() {
            let mut borrowed = Vec::new();
            let mut owned = Vec::new();
            assert_eq!(
                FrameVec::decode_into(&packet[..length], &mut borrowed).is_ok(),
                FrameVec::decode_owned_into(packet[..length].to_vec(), &mut owned).is_ok()
            );
        }
    }
}

#[cfg(test)]
mod scheduling_regressions {
    use super::*;

    #[test]
    fn cached_deadlines_match_a_full_scan_across_ack_loss_and_wrap() {
        let peer = "127.0.0.1:19132".parse().unwrap();
        let mut cached = SendQ::new(1400);
        cached.sequence_number = sequence::MASK - 80;
        let mut scanned = SendQ::new(1400);
        scanned.sequence_number = cached.sequence_number;
        let mut rng = 0x12345678u32;
        for tick in 0..4000 {
            rng = rng.wrapping_mul(1664525).wrapping_add(1013904223);
            if rng & 3 == 0 {
                for queue in [&mut cached, &mut scanned] {
                    queue
                        .insert(Reliability::ReliableOrdered, &[0xfe, 42])
                        .unwrap();
                }
            }
            if !cached.sent_packet.is_empty() && rng & 7 == 1 {
                let index = (rng as usize >> 8) % cached.sent_packet.len();
                let sequence = cached.sent_packet[index].0.sequence_number;
                for queue in [&mut cached, &mut scanned] {
                    if rng & 16 == 0 {
                        queue.ack(sequence, tick);
                    } else {
                        queue.nack(sequence, tick);
                    }
                }
            }
            // The oracle deliberately does the old full scan on every flush.
            scanned.next_retry = 0;
            scanned.retries_pending = true;
            let actual: Vec<_> = cached
                .flush(tick, &peer)
                .iter()
                .map(|f| f.serialize().unwrap())
                .collect();
            let expected: Vec<_> = scanned
                .flush(tick, &peer)
                .iter()
                .map(|f| f.serialize().unwrap())
                .collect();
            assert_eq!(actual, expected, "tick {tick}");
            assert_eq!(cached.buffered_bytes, scanned.buffered_bytes);
        }
    }

    #[test]
    fn lower_rto_invalidates_the_deadline_and_retries_keep_the_backoff() {
        let peer = "127.0.0.1:19132".parse().unwrap();
        let mut queue = SendQ::new(1400);
        queue.srtt = 133;
        queue.rto = 200;
        for _ in 0..2 {
            queue.insert(Reliability::ReliableOrdered, &[0xfe]).unwrap();
        }
        queue.flush(0, &peer);
        queue.ack(0, 50);
        assert_eq!(queue.rto, 174);
        assert!(queue.flush(173, &peer).is_empty());
        assert_eq!(queue.flush(174, &peer).len(), 1);
        assert!(queue.flush(434, &peer).is_empty());
        assert_eq!(queue.flush(435, &peer).len(), 1);
        queue.ack(1, 436);
        assert!(queue.is_empty());
    }

    #[test]
    fn ack_storage_is_recycled_without_replaying_old_ranges() {
        let mut set = ACKSet::default();
        let mut output = Vec::new();
        for index in 0..100 {
            set.insert(index);
            set.take_ack_into(&mut output);
            assert_eq!(output, [(index, index)]);
        }
        set.take_ack_into(&mut output);
        assert!(output.is_empty());
        assert!(set.ack.capacity() > 0);
    }
}

#[cfg(test)]
mod owned_payload_tests {
    use super::*;

    #[test]
    fn owned_sends_match_borrowed_wire_bytes_and_share_retry_storage() {
        let peer = "127.0.0.1:19132".parse().unwrap();
        for mode in [
            Reliability::Unreliable,
            Reliability::UnreliableSequenced,
            Reliability::Reliable,
            Reliability::ReliableOrdered,
            Reliability::ReliableSequenced,
        ] {
            for size in [64, 800, 4096] {
                let data = bytes::Bytes::from(vec![0xfe; size]);
                let mut owned = SendQ::new(1400);
                let mut borrowed = SendQ::new(1400);
                let a = owned.insert_bytes(mode, &data, 7);
                let b = borrowed.insert_with_order_channel(mode, &data, 7);
                assert_eq!(a.is_ok(), b.is_ok());
                if a.is_err() {
                    continue;
                }
                let frames = owned.flush(0, &peer);
                let expected = borrowed.flush(0, &peer);
                assert_eq!(
                    frames
                        .iter()
                        .map(|f| f.serialize().unwrap())
                        .collect::<Vec<_>>(),
                    expected
                        .iter()
                        .map(|f| f.serialize().unwrap())
                        .collect::<Vec<_>>()
                );
                let mut offset = 0;
                for frame in &frames {
                    assert_eq!(frame.data.as_ptr(), data[offset..].as_ptr());
                    offset += frame.data.len();
                }
                if mode != Reliability::Unreliable && mode != Reliability::UnreliableSequenced {
                    owned.nack(frames[0].sequence_number, 1);
                    let retry = owned.flush(1, &peer);
                    assert_eq!(retry[0].data.as_ptr(), data.as_ptr());
                    // Old ACK IDs still release each payload budget once.
                    owned.ack_ranges(&[(0, (frames.len() - 1) as u32)], 2);
                    assert!(owned.is_empty());
                    assert_eq!(owned.buffered_bytes, 0);
                }
            }
        }
    }
}
