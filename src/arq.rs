use std::collections::HashMap;

use crate::{datatype::*, utils::*, fragment::FragmentQ};


// if client send 1,2,3,4,5,6 to server ,then server maybe received
// UNRELIABLE - 5, 1, 6
// UNRELIABLE_SEQUENCED - 5 (6 was lost in transit, 1,2,3,4 arrived later than 5)
// RELIABLE - 5, 1, 4, 6, 2, 3
// RELIABLE_ORDERED - 1, 2, 3, 4, 5, 6
// RELIABLE_SEQUENCED - 5, 6 (1,2,3,4 arrived later than 5)

//With the UNRELIABLE_SEQUENCED transmission method, the game data does not need to arrive in every packet to avoid packet loss and retransmission, 
//because the new packet represents the new state, and the new state can be used directly, without waiting for the old packet to arrive. 
// TCP retransmission of lost packets will take a long time (1.5 times the ping value), causing network delay

#[derive(Clone)]
pub enum Reliability {
    // Unreliable packets are sent by straight UDP. They may arrive out of order, or not at all. This is best for data that is unimportant, or data that you send very frequently so even if some packets are missed newer packets will compensate.
    // Advantages - These packets don't need to be acknowledged by the network, saving the size of a UDP header in acknowledgment (about 50 bytes or so). The savings can really add up.
    // Disadvantages - No packet ordering, packets may never arrive, these packets are the first to get dropped if the send buffer is full.
    Unreliable = 0x00,
    // Unreliable sequenced packets are the same as unreliable packets, except that only the newest packet is ever accepted. Older packets are ignored. Advantages - Same low overhead as unreliable packets, and you don't have to worry about older packets changing your data to old values.
    // Disadvantages - A LOT of packets will be dropped since they may never arrive because of UDP and may be dropped even when they do arrive. These packets are the first to get dropped if the send buffer is full. The last packet sent may never arrive, which can be a problem if you stop sending packets at some particular point.
    UnreliableSequenced = 0x01,
    // Reliable packets are UDP packets monitored by a reliablilty layer to ensure they arrive at the destination.
    // Advantages - You know the packet will get there. Eventually...
    // Disadvantages - Retransmissions and acknowledgments can add significant bandwidth requirements. Packets may arrive very late if the network is busy. No packet ordering.
    Reliable = 0x02,
    // Reliable ordered packets are UDP packets monitored by a reliability layer to ensure they arrive at the destination and are ordered at the destination. Advantages - The packet will get there and in the order it was sent. These are by far the easiest to program for because you don't have to worry about strange behavior due to out of order or lost packets.
    // Disadvantages - Retransmissions and acknowledgments can add significant bandwidth requirements. Packets may arrive very late if the network is busy. One late packet can delay many packets that arrived sooner, resulting in significant lag spikes. However, this disadvantage can be mitigated by the clever use of ordering streams .
    ReliableOrdered = 0x03,
    // Reliable sequenced packets are UDP packets monitored by a reliability layer to ensure they arrive at the destination and are sequenced at the destination.
    // Advantages - You get the reliability of UDP packets, the ordering of ordered packets, yet don't have to wait for old packets. More packets will arrive with this method than with the unreliable sequenced method, and they will be distributed more evenly. The most important advantage however is that the latest packet sent will arrive, where with unreliable sequenced the latest packet sent may not arrive.
    // Disadvantages - Wasteful of bandwidth because it uses the overhead of reliable UDP packets to ensure late packets arrive that just get ignored anyway.
    ReliableSequenced = 0x04,
    UnreliableAckReceipt = 0x05,
    ReliableAckReceipt = 0x06,
    ReliableOrderedAckReceipt = 0x07,
    Unknown = 0xff,
}

pub fn is_reliable(flags : u8) -> bool {
    match transaction_reliability_id(flags){
        Reliability::Reliable => true,
        Reliability::ReliableOrdered => true, 
        Reliability::ReliableSequenced => true,
        _ => false
    }
}

pub fn is_sequenced_or_ordered(flags : u8) -> bool {

    match transaction_reliability_id(flags){
        Reliability::UnreliableSequenced => true,
        Reliability::ReliableOrdered => true, 
        Reliability::ReliableSequenced => true,
        _ => false
    }
}

pub fn is_sequenced(flags : u8) -> bool {

    match transaction_reliability_id(flags){
        Reliability::UnreliableSequenced => true,
        Reliability::ReliableSequenced => true, 
        _ => false
    }
}

pub fn transaction_reliability_id(id : u8) -> Reliability {
    match id{
        0x00 => Reliability::Unreliable,
        0x01 => Reliability::UnreliableSequenced,
        0x02 => Reliability::Reliable,
        0x03 => Reliability::ReliableOrdered,
        0x04 => Reliability::ReliableSequenced,
        0x05 => Reliability::UnreliableAckReceipt,
        0x06 => Reliability::ReliableAckReceipt,
        0x07 => Reliability::ReliableOrderedAckReceipt,
        _ => Reliability::Unknown
    }
}

pub fn transaction_reliability_id_to_u8(packetid : Reliability) -> u8 {
    match packetid{
        Reliability::Unreliable => 0x00,
        Reliability::UnreliableSequenced => 0x01,
        Reliability::Reliable => 0x02,
        Reliability::ReliableOrdered => 0x03,
        Reliability::ReliableSequenced => 0x04,
        Reliability::UnreliableAckReceipt => 0x05,
        Reliability::ReliableAckReceipt => 0x06,
        Reliability::ReliableOrderedAckReceipt => 0x07,
        Reliability::Unknown => 0xff,
    }
}


#[derive(Clone)]
pub struct FrameSetPacket {
    pub id : u8, 
    pub sequence_number : u32,
    pub flags : u8 , 
    pub length_in_bytes : u16 ,
    pub reliable_frame_index : u32,
    pub sequenced_frame_index : u32, 
    pub ordered_frame_index : u32,
    pub order_channel : u8 ,
    pub compound_size : u32,
    pub compound_id : u16,
    pub fragment_index : u32,
    pub data : Vec<u8>
}

impl FrameSetPacket {

    pub fn new(r: Reliability , data : Vec<u8>) -> FrameSetPacket{
        let flag = transaction_reliability_id_to_u8(r) << 5;

        FrameSetPacket{
            id : 0,
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
            data: data
        }
    }

    pub async fn _deserialize(buf : Vec<u8>) -> (Self , bool){

        let mut reader = RaknetReader::new(buf);

        let mut ret = Self{
            id : 0,
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
            data: vec![]
        };

        ret.id = reader.read_u8().await.unwrap();
        ret.sequence_number = reader.read_u24(Endian::Little).await.unwrap();

        ret.flags = reader.read_u8().await.unwrap();

        //Top 3 bits are reliability type
        //224 = 1110 0000(b)
        let real_flag = (ret.flags & 224) >> 5;
        ret.length_in_bytes = reader.read_u16(Endian::Big).await.unwrap()/8;
    
        if is_reliable(real_flag){
            ret.reliable_frame_index = reader.read_u24(Endian::Little).await.unwrap();
        }
        
        if is_sequenced(real_flag) {
            ret.sequenced_frame_index = reader.read_u24(Endian::Little).await.unwrap();
        }
        if is_sequenced_or_ordered(real_flag){
            ret.ordered_frame_index = reader.read_u24(Endian::Little).await.unwrap();
            ret.order_channel = reader.read_u8().await.unwrap();
        }
    
        //fourth bit is 1 when the frame is fragmented and part of a compound.
        //flags and 8 [0000 1000(b)] == if fragmented
        if (ret.flags & 8) != 0 {
            ret.compound_size = reader.read_u32(Endian::Big).await.unwrap();
            ret.compound_id = reader.read_u16(Endian::Big).await.unwrap();
            ret.fragment_index = reader.read_u32(Endian::Big).await.unwrap();
        }
    
        let mut buf = vec![0u8 ; ret.length_in_bytes as usize].into_boxed_slice();
        reader.read(&mut buf).await.unwrap();
        ret.data.append(&mut buf.to_vec());

        (ret , reader.pos() == buf.len() as u64)
    }

    pub async fn serialize(&self) -> Vec<u8>{
        let mut writer = RaknetWriter::new();

        let id = 0x80 | 4;

        writer.write_u8(id).await.unwrap();
        writer.write_u24(self.sequence_number , Endian::Little).await.unwrap();
        
        writer.write_u8(self.flags).await.unwrap();
        writer.write_u16(self.length_in_bytes*8 ,Endian::Big).await.unwrap();
    
        //Top 3 bits are reliability type
        //224 = 1110 0000(b)
        let real_flag = (self.flags & 224) >> 5;

        if is_reliable(real_flag){
            writer.write_u24(self.reliable_frame_index, Endian::Little).await.unwrap();
        }
        
        if is_sequenced(real_flag) {
            writer.write_u24(self.sequenced_frame_index ,Endian::Little).await.unwrap();
        }
        if is_sequenced_or_ordered(real_flag){
            writer.write_u24(self.ordered_frame_index , Endian::Little).await.unwrap();
            writer.write_u8(self.order_channel).await.unwrap();
        }
    
        //fourth bit is 1 when the frame is fragmented and part of a compound.
        //flags and 8 [0000 1000(b)] == if fragmented
        if (self.flags & 0x08) != 0 {
            writer.write_u32(self.compound_size , Endian::Big).await.unwrap();
            writer.write_u16(self.compound_id , Endian::Big).await.unwrap();
            writer.write_u32(self.fragment_index , Endian::Big).await.unwrap();
        }
        writer.write(&self.data.as_slice()).await.unwrap();

        writer.get_raw_payload()
    }

    pub fn is_fragment(&self) -> bool{
        (self.flags & 0x08) != 0
    }

    pub fn _reliable(&self) -> Reliability{
        transaction_reliability_id((self.flags & 224) >> 5)
    }

    pub fn _size(&self) -> usize{
        let mut ret = 0;
        // id
        ret += 1;
        // sequence number
        ret += 3;
        // flags
        ret += 1;
        // length_in_bits
        ret += 2;

        let real_flag = (self.flags & 224) >> 5;

        if is_reliable(real_flag) {
            // reliable frame index
            ret += 3;
        }
        if is_sequenced(real_flag) {
            // sequenced frame index
            ret += 3;
        }
        if is_sequenced_or_ordered(real_flag) {
            //ordered frame index + order channel
            ret += 4;
        }
        if (self.flags & 8 ) != 0{
            //compound size + compound id + fragment index
            ret += 10;
        }
        //body
        ret += self.data.len();
        ret
    }
}

pub struct FrameVec{
    pub id : u8, 
    pub sequence_number : u32,
    pub frames : Vec<FrameSetPacket>
}

impl FrameVec {
    pub async fn new(buf : Vec<u8>) -> Self{

        let mut ret = Self{
            id: 0,
            sequence_number: 0,
            frames: vec![],
        };

        let size = buf.len();

        let mut reader = RaknetReader::new(buf);

        ret.id = reader.read_u8().await.unwrap();
        ret.sequence_number = reader.read_u24(Endian::Little).await.unwrap();

        while reader.pos() < size.try_into().unwrap(){
            let mut frame = FrameSetPacket{
                id : 0,
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
                data: vec![]
            };
    
            frame.flags = reader.read_u8().await.unwrap();
    
            //Top 3 bits are reliability type
            //224 = 1110 0000(b)
            let real_flag = (frame.flags & 224) >> 5;
            frame.length_in_bytes = reader.read_u16(Endian::Big).await.unwrap()/8;
        
            if is_reliable(real_flag){
                frame.reliable_frame_index = reader.read_u24(Endian::Little).await.unwrap();
            }
            
            if is_sequenced(real_flag) {
                frame.sequenced_frame_index = reader.read_u24(Endian::Little).await.unwrap();
            }
            if is_sequenced_or_ordered(real_flag){
                frame.ordered_frame_index = reader.read_u24(Endian::Little).await.unwrap();
                frame.order_channel = reader.read_u8().await.unwrap();
            }
        
            //fourth bit is 1 when the frame is fragmented and part of a compound.
            //flags and 8 [0000 1000(b)] == if fragmented
            if (frame.flags & 8) != 0 {
                frame.compound_size = reader.read_u32(Endian::Big).await.unwrap();
                frame.compound_id = reader.read_u16(Endian::Big).await.unwrap();
                frame.fragment_index = reader.read_u32(Endian::Big).await.unwrap();
            }
        
            let mut buf = vec![0u8 ; frame.length_in_bytes as usize].into_boxed_slice();
            reader.read(&mut buf).await.unwrap();
            frame.data.append(&mut buf.to_vec());
            ret.frames.push(frame);
        }

        ret
    }

    pub async fn _serialize(&self) -> Vec<u8>{
        let mut writer = RaknetWriter::new();

        let id = 0x80 | 4 | 8;

        writer.write_u8(id).await.unwrap();
        writer.write_u24(self.sequence_number , Endian::Little).await.unwrap();
        
        for frame in &self.frames{
            writer.write_u8(frame.flags).await.unwrap();
            writer.write_u16(frame.length_in_bytes*8 ,Endian::Big).await.unwrap();
        
            //Top 3 bits are reliability type
            //224 = 1110 0000(b)
            let real_flag = (frame.flags & 224) >> 5;
    
            if is_reliable(real_flag){
                writer.write_u24(frame.reliable_frame_index, Endian::Little).await.unwrap();
            }
            
            if is_sequenced(real_flag) {
                writer.write_u24(frame.sequenced_frame_index ,Endian::Little).await.unwrap();
            }
            if is_sequenced_or_ordered(real_flag){
                writer.write_u24(frame.ordered_frame_index , Endian::Little).await.unwrap();
                writer.write_u8(frame.order_channel).await.unwrap();
            }
        
            //fourth bit is 1 when the frame is fragmented and part of a compound.
            //flags and 8 [0000 1000(b)] == if fragmented
            if (frame.flags & 0x08) != 0 {
                writer.write_u32(frame.compound_size , Endian::Big).await.unwrap();
                writer.write_u16(frame.compound_id , Endian::Big).await.unwrap();
                writer.write_u32(frame.fragment_index , Endian::Big).await.unwrap();
            }
            writer.write(&frame.data.as_slice()).await.unwrap();
        }


        writer.get_raw_payload()
    }
}

pub struct ACKSet{
    max_sequence_number : u32,
    min_sequence_number : u32,
    nack : Vec<u32>
}

impl ACKSet {
    pub fn new() -> Self{
        ACKSet{
            max_sequence_number : 0,
            min_sequence_number : 0,
            nack : vec![]
        }
    }
    pub async fn insert(&mut self, s : u32) {
        self.nack.retain(|x| s != *x);

        if s == self.max_sequence_number + 1 {
            self.max_sequence_number = s;
        } else {
            // old packet
            if s <= self.max_sequence_number {
                return;
            }

            // nack
            for i in self.max_sequence_number + 1..s{
                self.nack.push(i);
            }
            self.max_sequence_number = s;
        }
    }

    pub async fn get_nack(&self) -> Vec<u32>{
        self.nack.clone()
    }

    pub async fn get_ack(&mut self) -> Vec<(u32 , u32)> {

        let mut ret = vec![(self.min_sequence_number , self.max_sequence_number)];

        self.nack.sort();

        for i in &self.nack {

            for j in 0..ret.len() {
                if *i >= ret[j].0 && *i <= ret[j].1{
                    let tmp1 = (ret[j].0 , *i - 1);
                    let tmp2 = (*i + 1 , ret[j].1);
                    ret.remove(j);

                    if tmp1.1 >= tmp1.0{
                        ret.push(tmp1);
                    }
                    if tmp2.1 >= tmp2.0{
                        ret.push(tmp2);
                    }
                    
                    break;
                }
            }
        }

        ret
    }

    pub async fn _reset(&mut self) {
        self.max_sequence_number = 0;
        self.min_sequence_number = 0;
        self.nack.clear();
    }
}

pub struct RecvQ{
    max_order_index : u32,
    old_order_index : u32,
    packets : HashMap<u32 , FrameSetPacket>,
    fragment_queue : FragmentQ
}

impl RecvQ {
    pub fn new() -> Self{
        Self{
            packets: HashMap::new(),
            fragment_queue: FragmentQ::new(),
            max_order_index: 0,
            old_order_index: 0,
        }
    }

    pub fn insert(&mut self , frame : FrameSetPacket) {
        if frame.is_fragment() {

            if frame.ordered_frame_index >= self.max_order_index {
                self.max_order_index = frame.ordered_frame_index + 1;
            }

            self.fragment_queue.insert(frame);

            for i in self.fragment_queue.flush(){
                self.packets.insert(i.ordered_frame_index , i);
            }
            return;
        }

        if self.packets.contains_key(&frame.ordered_frame_index) {
            return;
        }

        if self.old_order_index > frame.ordered_frame_index {
            return;
        }

        if frame.ordered_frame_index >= self.max_order_index {
            self.max_order_index = frame.ordered_frame_index + 1;
        }
        self.packets.insert(frame.ordered_frame_index, frame);

    }

    pub fn flush(&mut self ) -> Vec<FrameSetPacket>{
        let mut ret = vec![];
        let mut index = self.old_order_index;
        for i in self.old_order_index..self.max_order_index {
            if self.packets.contains_key(&i) {
                ret.push(self.packets.get(&i).unwrap().clone());
            } else {
                break;
            }
            index += 1;
        }
        self.old_order_index = index;
        self.packets.clear();
        ret

    }
}

pub struct SendQ{
    pub current_sequence_number : u32,
    //packet : FrameSetPacket , is_sent: bool ,last_tick : i64 
    pub packets : HashMap<u32, (FrameSetPacket , bool , i64)>,
    //packet : FrameSetPacket , last_tick : i64 
    pub repeat_request : HashMap<u32, (FrameSetPacket , i64)>,
}

impl SendQ{
    pub fn new() -> Self{
        Self{
            current_sequence_number : 0,
            packets: HashMap::new(),
            repeat_request: HashMap::new(),
        }
    }

    pub fn insert(&mut self, frame : FrameSetPacket , tick :i64){
        if frame.sequence_number == self.current_sequence_number {
            self.packets.insert(frame.sequence_number, (frame , false , tick));
            self.current_sequence_number += 1;
        }
    }

    pub fn nack(&mut self , sequence_number : u32 , tick : i64){
        if self.packets.contains_key(&sequence_number){
            let p = self.packets.get(&sequence_number).unwrap();
            self.repeat_request.insert(p.0.sequence_number, (p.0.clone() ,tick));
            self.packets.remove(&sequence_number);
        }
    }

    pub fn ack(&mut self , sequence_number : u32){
        if self.packets.contains_key(&sequence_number) {
            self.packets.remove(&sequence_number);
        }

        if self.repeat_request.contains_key(&sequence_number) {
            self.repeat_request.remove(&sequence_number);
        }
    }

    pub fn tick(&mut self , tick : i64){
        let keys : Vec<u32> = self.packets.keys().cloned().collect();

        for i in keys{
            let p = self.packets.get_mut(&i).unwrap();
            
            // RTO = 1000ms
            if tick - p.2 >= 1000 && !self.repeat_request.contains_key(&i){
                self.repeat_request.insert(i, (p.0.clone() ,tick));
            }
        }
    }

    pub fn flush(&mut self) -> Vec<FrameSetPacket> {

        let mut ret = vec![];

        if !self.repeat_request.is_empty(){
            for i in self.repeat_request.keys(){
                ret.push(self.repeat_request.get(i).unwrap().0.clone());
            }
            return ret;
        }

        if !self.packets.is_empty(){

            let mut keys : Vec<u32> = self.packets.keys().cloned().collect();
            keys.sort();

            for i in keys{
                let p = self.packets.get_mut(&i).unwrap();
                if !p.1{
                    ret.push(p.0.clone());
                    p.1 = true;
                }
            }
        }
        return ret;
    }
}

#[tokio::test]
async fn test_get_acks(){

    let mut ackset = ACKSet::new();
    ackset.insert(0).await;
    ackset.insert(1).await;
    ackset.insert(2).await;
    ackset.insert(4).await;
    ackset.insert(2).await;

    assert!(ackset.get_nack().await == vec![3]);

    ackset.insert(3).await;

    assert!(ackset.get_nack().await == vec![]);

    ackset._reset().await;

    let mut ackset = ACKSet::new();
    ackset.insert(0).await;
    ackset.insert(1).await;
    ackset.insert(2).await;
    ackset.insert(4).await;

    let acks = ackset.get_ack().await;

    assert!(acks == vec![(0,2), (4,4)]);

    ackset._reset().await;

    ackset.insert(0).await;
    ackset.insert(1).await;
    ackset.insert(2).await;
    ackset.insert(6).await;

    let acks = ackset.get_ack().await;

    assert!(acks == vec![(0,2), (6,6)]);

    ackset._reset().await;

    ackset.insert(0).await;
    ackset.insert(1).await;
    ackset.insert(2).await;
    ackset.insert(5).await;
    ackset.insert(8).await;

    assert!(ackset.get_nack().await == vec![3, 4, 6, 7]);

    let acks = ackset.get_ack().await;

    assert!(acks == vec![(0,2), (5,5) , (8,8)]);

    ackset._reset().await;

    ackset.insert(0).await;
    ackset.insert(1).await;
    ackset.insert(2).await;
    ackset.insert(4).await;
    ackset.insert(8).await;
    ackset.insert(9).await;
    ackset.insert(11).await;
    ackset.insert(6).await;

    assert!(ackset.get_nack().await == vec![3, 5, 7, 10]);

    let acks = ackset.get_ack().await;

    assert!(acks == vec![(0,2) , (4, 4) , (6,6) , (8,9) , (11,11)]);

}

#[tokio::test]
async fn test_frame_serialize_deserialize(){

    //minecraft 1.18.12 first frame packet
    let p : Vec<u8> = [
        132,
        0,
        0,
        0,
        64,
        0,
        144,
        0,
        0,
        0,
        9,
        146,
        33,
        7,
        47,
        57,
        18,
        128,
        111,
        0,
        0,
        0,
        0,
        20,
        200,
        47,
        41,
        0,
    ].to_vec();

    let a = FrameSetPacket::_deserialize(p.clone()).await;
    assert!(a.0.serialize().await == p);

}

#[tokio::test]
async fn test_recvq(){
    let mut r = RecvQ::new();
    let mut p = FrameSetPacket::new(Reliability::Reliable, vec![]);
    p.sequence_number = 0;
    p.ordered_frame_index = 0;
    r.insert(p);

    let mut p = FrameSetPacket::new(Reliability::Reliable, vec![]);
    p.sequence_number = 1;
    p.ordered_frame_index = 1;
    r.insert(p);

    let ret = r.flush();
    assert!(ret.len() == 2);
}

#[tokio::test]
async fn test_recvq_fragment(){
    let mut r = RecvQ::new();
    let mut p = FrameSetPacket::new(Reliability::Reliable, vec![1]);
    p.flags |= 8;
    p.sequence_number = 0;
    p.ordered_frame_index = 0;
    p.compound_id = 1;
    p.compound_size = 3;
    p.fragment_index = 1;
    r.insert(p);

    let mut p = FrameSetPacket::new(Reliability::Reliable, vec![2]);
    p.flags |= 8;
    p.sequence_number = 1;
    p.ordered_frame_index = 1;
    p.compound_id = 1;
    p.compound_size = 3;
    p.fragment_index = 2;
    r.insert(p);

    let mut p = FrameSetPacket::new(Reliability::Reliable, vec![3]);
    p.flags |= 8;
    p.sequence_number = 2;
    p.ordered_frame_index = 2;
    p.compound_id = 1;
    p.compound_size = 3;
    p.fragment_index = 3;
    r.insert(p);

    let ret = r.flush();
    assert!(ret.len() == 1);
    assert!(ret[0].data == vec![1,2,3]);
}

#[tokio::test]
async fn test_sendq(){
    let mut s = SendQ::new();
    let mut p = FrameSetPacket::new(Reliability::Reliable, vec![]);
    p.sequence_number = 0;
    s.insert(p, 0);

    let mut p = FrameSetPacket::new(Reliability::Reliable, vec![]);
    p.sequence_number = 1;
    s.insert(p, 50);

    let ret = s.flush();
    assert!(ret.len() == 2);

    s.tick(1000);

    let ret = s.flush();
    assert!(ret.len() == 1);

    s.tick(1050);

    let ret = s.flush();
    assert!(ret.len() == 2);

    s.ack(0);

    let ret = s.flush();
    assert!(ret.len() == 1);

    s.ack(1);

    let ret = s.flush();
    assert!(ret.len() == 0);
}