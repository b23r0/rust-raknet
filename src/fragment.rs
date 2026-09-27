use crate::arq::{FrameSetPacket, Reliability};
use crate::error::*;
use std::collections::HashMap;

struct Fragment {
    pub flags: u8,
    pub compound_size: u32,
    pub ordered_frame_index: u32,
    pub order_channel: u8,
    pub frames: HashMap<u32, FrameSetPacket>,
}

impl Fragment {
    pub fn new(flags: u8, compound_size: u32, ordered_frame_index: u32, order_channel: u8) -> Self {
        Self {
            flags,
            compound_size,
            ordered_frame_index,
            order_channel,
            frames: HashMap::new(),
        }
    }

    pub fn full(&self) -> bool {
        self.compound_size != 0 && self.frames.len() == self.compound_size as usize
    }

    pub fn insert(&mut self, frame: FrameSetPacket) {
        if self.full() {
            return;
        }

        if frame.fragment_index >= self.compound_size
            || frame.compound_size != self.compound_size
            || frame.order_channel != self.order_channel
            || frame.ordered_frame_index != self.ordered_frame_index
            || (frame.flags & 0xe0) != (self.flags & 0xe0)
        {
            return;
        }

        self.frames.entry(frame.fragment_index).or_insert(frame);
    }

    pub fn merge(&self) -> Result<FrameSetPacket> {
        if !self.full() {
            return Err(RaknetError::PacketParseError);
        }

        let mut keys: Vec<u32> = self.frames.keys().copied().collect();
        keys.sort_unstable();

        let last_key = keys.last().ok_or(RaknetError::PacketParseError)?;
        let sequence_number = self.frames[last_key].sequence_number;
        let capacity = keys
            .iter()
            .try_fold(0usize, |total, index| {
                total.checked_add(self.frames[index].data.len())
            })
            .ok_or(RaknetError::PacketSizeExceedMTU)?;
        let mut data = Vec::with_capacity(capacity);
        for index in keys {
            data.extend_from_slice(&self.frames[&index].data);
        }

        let mut ret = FrameSetPacket::new(Reliability::from((self.flags & 224) >> 5)?, data);

        ret.ordered_frame_index = self.ordered_frame_index;
        ret.order_channel = self.order_channel;
        ret.sequence_number = sequence_number;
        Ok(ret)
    }
}

#[derive(Default)]
pub struct FragmentQ {
    fragments: HashMap<u16, Fragment>,
}

impl FragmentQ {
    pub fn insert(&mut self, frame: FrameSetPacket) {
        let fragment = self.fragments.entry(frame.compound_id).or_insert_with(|| {
            Fragment::new(
                frame.flags,
                frame.compound_size,
                frame.ordered_frame_index,
                frame.order_channel,
            )
        });
        fragment.insert(frame);
    }

    pub fn flush(&mut self) -> Result<Vec<FrameSetPacket>> {
        let mut ret = vec![];

        let completed: Vec<u16> = self
            .fragments
            .iter()
            .filter_map(|(&compound_id, fragment)| fragment.full().then_some(compound_id))
            .collect();

        for compound_id in completed {
            if let Some(fragment) = self.fragments.remove(&compound_id) {
                ret.push(fragment.merge()?);
            }
        }

        Ok(ret)
    }

    pub fn size(&self) -> usize {
        self.fragments.len()
    }
}
