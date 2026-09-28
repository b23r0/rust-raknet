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
}

impl FragmentQ {
    fn insert_frame(&mut self, frame: FrameSetPacket) -> u16 {
        let compound_id = frame.compound_id;
        let fragment = self.fragments.entry(compound_id).or_insert_with(|| {
            Fragment::new(
                frame.flags,
                frame.compound_size,
                frame.ordered_frame_index,
                frame.order_channel,
            )
        });
        fragment.insert(frame);
        compound_id
    }

    pub(crate) fn insert_and_take_completed(
        &mut self,
        frame: FrameSetPacket,
    ) -> Result<Option<FrameSetPacket>> {
        let compound_id = self.insert_frame(frame);
        if self.fragments.get(&compound_id).is_some_and(Fragment::full) {
            self.fragments
                .remove(&compound_id)
                .map(|fragment| fragment.merge())
                .transpose()
        } else {
            Ok(None)
        }
    }

    pub(crate) fn size(&self) -> usize {
        self.fragments.len()
    }
}
