use std::{collections::HashMap};

use crate::arq::{FrameSetPacket, transaction_reliability_id};

struct Fragment{
    pub flags : u8,
    pub compound_size : u32 ,
    pub ordered_frame_index : u32,
    pub frames : HashMap<u32,FrameSetPacket>,
}

impl Fragment {
    pub fn new(
        flags: u8 , 
        compound_size : u32 , 
        ordered_frame_index : u32) -> Self{

        Self{
            flags,
            compound_size,
            ordered_frame_index,
            frames: HashMap::new(),
        }
    }

    pub fn full(&self) -> bool{
        self.frames.len() == self.compound_size as usize
    }

    pub fn insert(&mut self , frame : FrameSetPacket) {
        if self.full(){
            return;
        }

        if self.frames.contains_key(&frame.fragment_index) {
            return;
        }

        self.frames.insert(frame.fragment_index, frame);
    }

    pub fn merge(&mut self) -> FrameSetPacket{
        let mut buf = vec![];

        let mut keys : Vec<u32> = self.frames.keys().cloned().collect();

        keys.sort();

        for i in keys{
            buf.append(&mut self.frames[&i].data.clone());
        }

        let mut ret = FrameSetPacket::new(
            transaction_reliability_id((self.flags & 224) >> 5) , 
            buf
        );

        ret.ordered_frame_index = self.ordered_frame_index;
        ret
    }
}

pub struct FragmentQ{
    fragments : HashMap<u16, Fragment>,
}

impl FragmentQ {
    
    pub fn new() -> Self{
        Self{
            fragments : HashMap::new()
        }
    }

    pub fn insert(&mut self , frame : FrameSetPacket){
        if self.fragments.contains_key(&frame.compound_id){
            self.fragments.get_mut(&frame.compound_id).unwrap().insert(frame);
        } else {
            let mut v = Fragment::new(frame.flags, frame.compound_size, frame.ordered_frame_index);
            let k = frame.compound_id;
            v.insert(frame);
            self.fragments.insert(k, v);
        }
    }

    pub fn flush(&mut self) -> Vec<FrameSetPacket>{
        let mut ret = vec![];

        let keys : Vec<u16> = self.fragments.keys().cloned().collect();

        for i in keys{
            let a = self.fragments.get_mut(&i).unwrap();
            if a.full() {
                ret.push(a.merge());
            }
        }
        ret
    }
}