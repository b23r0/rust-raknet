use crate::arq::{FrameSetPacket, Reliability};
use crate::error::*;
use std::collections::HashMap;
use std::time::{Duration, Instant};

pub(crate) const MAX_FRAGMENTS: usize = 65_536;

struct Fragment {
    pub flags: u8,
    pub compound_size: u32,
    pub ordered_frame_index: u32,
    pub order_channel: u8,
    pub frames: HashMap<u32, FrameSetPacket>,
    bytes: usize,
    updated: Instant,
}

impl Fragment {
    pub fn new(flags: u8, compound_size: u32, ordered_frame_index: u32, order_channel: u8) -> Self {
        Self {
            flags,
            compound_size,
            ordered_frame_index,
            order_channel,
            frames: HashMap::new(),
            bytes: 0,
            updated: Instant::now(),
        }
    }

    pub fn full(&self) -> bool {
        self.compound_size != 0 && self.frames.len() == self.compound_size as usize
    }

    pub fn insert(&mut self, frame: FrameSetPacket) -> Result<usize> {
        if self.full() {
            return Ok(0);
        }

        if frame.fragment_index >= self.compound_size
            || frame.compound_size != self.compound_size
            || frame.order_channel != self.order_channel
            || frame.ordered_frame_index != self.ordered_frame_index
            || (frame.flags & 0xe0) != (self.flags & 0xe0)
        {
            return Err(RaknetError::PacketParseError);
        }

        if let std::collections::hash_map::Entry::Vacant(entry) =
            self.frames.entry(frame.fragment_index)
        {
            let bytes = frame.data.len() + 128;
            entry.insert(frame);
            self.bytes += bytes;
            self.updated = Instant::now();
            Ok(bytes)
        } else {
            Ok(0)
        }
    }

    pub fn merge(&self) -> Result<FrameSetPacket> {
        if !self.full() {
            return Err(RaknetError::PacketParseError);
        }

        let last_index = self
            .compound_size
            .checked_sub(1)
            .ok_or(RaknetError::PacketParseError)?;
        let sequence_number = self
            .frames
            .get(&last_index)
            .ok_or(RaknetError::PacketParseError)?
            .sequence_number;
        let capacity = (0..self.compound_size)
            .try_fold(0usize, |total, index| {
                total.checked_add(self.frames.get(&index)?.data.len())
            })
            .ok_or(RaknetError::PacketSizeExceedMTU)?;
        let mut data = Vec::with_capacity(capacity);
        for index in 0..self.compound_size {
            let frame = self
                .frames
                .get(&index)
                .ok_or(RaknetError::PacketParseError)?;
            data.extend_from_slice(&frame.data);
        }

        let mut ret = FrameSetPacket::new(Reliability::from((self.flags & 224) >> 5)?, data);

        ret.ordered_frame_index = self.ordered_frame_index;
        ret.order_channel = self.order_channel;
        ret.sequence_number = sequence_number;
        Ok(ret)
    }
}

#[derive(Default)]
pub(crate) struct FragmentQ {
    fragments: HashMap<u16, Fragment>,
    bytes: usize,
}

impl FragmentQ {
    fn insert_frame(&mut self, frame: FrameSetPacket) -> Result<u16> {
        let compound_id = frame.compound_id;
        if frame.compound_size == 0
            || frame.compound_size as usize > MAX_FRAGMENTS
            || self.bytes.saturating_add(frame.data.len() + 128) > 64 * 1024 * 1024
            || (self.fragments.len() >= 1024 && !self.fragments.contains_key(&compound_id))
        {
            return Err(RaknetError::PacketParseError);
        }
        let fragment = self.fragments.entry(compound_id).or_insert_with(|| {
            Fragment::new(
                frame.flags,
                frame.compound_size,
                frame.ordered_frame_index,
                frame.order_channel,
            )
        });
        self.bytes += fragment.insert(frame)?;
        Ok(compound_id)
    }

    pub(crate) fn insert_and_take_completed(
        &mut self,
        frame: FrameSetPacket,
    ) -> Result<Option<FrameSetPacket>> {
        let compound_id = self.insert_frame(frame)?;
        if self.fragments.get(&compound_id).is_some_and(Fragment::full) {
            self.fragments
                .remove(&compound_id)
                .map(|fragment| {
                    self.bytes -= fragment.bytes;
                    fragment.merge()
                })
                .transpose()
        } else {
            Ok(None)
        }
    }

    pub(crate) fn size(&self) -> usize {
        self.fragments.len()
    }

    pub(crate) fn expired(&self) -> bool {
        self.fragments
            .values()
            .any(|fragment| fragment.updated.elapsed() >= Duration::from_secs(60))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fragment(index: u32) -> FrameSetPacket {
        let mut frame = FrameSetPacket::new(Reliability::ReliableOrdered, vec![0xfe]);
        frame.flags |= 16;
        frame.compound_size = 2;
        frame.fragment_index = index;
        frame
    }

    #[test]
    fn incomplete_fragments_expire_and_completed_ones_release_the_budget() {
        let mut queue = FragmentQ::default();
        queue.insert_and_take_completed(fragment(0)).unwrap();
        assert!(!queue.expired());
        assert!(queue.bytes > 1);
        queue.fragments.get_mut(&0).unwrap().updated = Instant::now() - Duration::from_secs(61);
        assert!(queue.expired());
        assert!(
            queue
                .insert_and_take_completed(fragment(1))
                .unwrap()
                .is_some()
        );
        assert_eq!(queue.bytes, 0);
        assert!(!queue.expired());
    }

    #[test]
    fn conflicting_compound_metadata_is_rejected() {
        let mut queue = FragmentQ::default();
        queue.insert_and_take_completed(fragment(0)).unwrap();
        let mut frame = fragment(1);
        frame.order_channel = 2;
        assert!(queue.insert_and_take_completed(frame).is_err());
    }
}
