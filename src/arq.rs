use crate::{packet::*, datatype::*, utils::*};


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

pub struct Frame{
    pub id : u8, 
    pub sequence_number : u32,
    pub data : Vec<FrameSetPacket>
}

impl Frame {
    pub async fn deserialize(buf : Vec<u8>) -> Self{

        let size = buf.len();
        let mut reader = RaknetReader::new(buf);
        let id = reader.read_u8().await.unwrap();
        let sequence_number = reader.read_u24(Endian::Little).await.unwrap();

        let mut data : Vec<FrameSetPacket> = vec![];

        while reader.pos() < size as u64{

            let mut ret = init_packet_frame_set_packet().await;

            ret.flags = reader.read_u8().await.unwrap();

            //Top 3 bits are reliability type, fourth bit is 1 when the frame is fragmented and part of a compound.
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
        
            
            //flags and 1000(b) == is fragmented
            if (ret.flags & 0x08) != 0 {
                ret.compound_size = reader.read_u32(Endian::Big).await.unwrap();
                ret.compound_id = reader.read_u16(Endian::Big).await.unwrap();
                ret.fragment_index = reader.read_u32(Endian::Big).await.unwrap();
            }
        
            let mut buf = vec![0u8 ; ret.length_in_bytes as usize].into_boxed_slice();
            reader.read(&mut buf).await.unwrap();
            ret.data.append(&mut buf.to_vec());

            data.push(ret);
        }

        Frame{
            id,
            sequence_number,
            data: data,
        }
    }

    pub async fn serialize(&self) -> Vec<u8>{
        let mut writer = RaknetWriter::new();

        writer.write_u8(self.id).await.unwrap();
        writer.write_u24(self.sequence_number , Endian::Little).await.unwrap();
        
        for packet in &self.data{
            writer.write_u8(packet.flags).await.unwrap();
            writer.write_u16(packet.length_in_bytes*8 ,Endian::Big).await.unwrap();
        
            //Top 3 bits are reliability type, fourth bit is 1 when the frame is fragmented and part of a compound.
            let real_flag = (packet.flags & 224) >> 5;

            if is_reliable(real_flag){
                writer.write_u24(packet.reliable_frame_index, Endian::Little).await.unwrap();
            }
            
            if is_sequenced(real_flag) {
                writer.write_u24(packet.sequenced_frame_index ,Endian::Little).await.unwrap();
            }
            if is_sequenced_or_ordered(real_flag){
                writer.write_u24(packet.ordered_frame_index , Endian::Little).await.unwrap();
                writer.write_u8(packet.order_channel).await.unwrap();
            }
        
            //flags and 1000(b) == is fragmented
            if (packet.flags & 0x08) != 0 {
                writer.write_u32(packet.compound_size , Endian::Big).await.unwrap();
                writer.write_u16(packet.compound_id , Endian::Big).await.unwrap();
                writer.write_u32(packet.fragment_index , Endian::Big).await.unwrap();
            }
            writer.write(&packet.data.as_slice()).await.unwrap();
        }

        writer.get_raw_payload()
    }
}

pub struct ACKSet{
    max : u32,
    min : u32,
    nack : Vec<u32>
}

impl ACKSet {
    pub fn new() -> Self{
        ACKSet{
            max : 0,
            min : 0,
            nack : vec![]
        }
    }
    pub async fn insert(&mut self, s : u32) {
        self.nack.retain(|x| s != *x);

        if s == self.max + 1 {
            self.max = s;
        } else {
            // old packet
            if s <= self.max {
                return;
            }

            // nack
            for i in self.max + 1..s{
                self.nack.push(i);
            }
            self.max = s;
        }
    }

    pub async fn get_nack(&self) -> Vec<u32>{
        self.nack.clone()
    }

    pub async fn get_ack(&mut self) -> Vec<(u32 , u32)> {

        let mut ret = vec![(self.min , self.max)];

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

    pub async fn reset(&mut self) {
        self.max = 0;
        self.min = 0;
        self.nack.clear();
    }
}

#[tokio::test]
async fn test_get_acks(){

    let mut ackset = ACKSet::new();
    ackset.insert(0).await;
    ackset.insert(1).await;
    ackset.insert(2).await;
    ackset.insert(4).await;

    assert!(ackset.get_nack().await == vec![3]);

    ackset.insert(3).await;

    assert!(ackset.get_nack().await == vec![]);

    ackset.reset().await;

    let mut ackset = ACKSet::new();
    ackset.insert(0).await;
    ackset.insert(1).await;
    ackset.insert(2).await;
    ackset.insert(4).await;

    let acks = ackset.get_ack().await;

    assert!(acks == vec![(0,2), (4,4)]);

    ackset.reset().await;

    ackset.insert(0).await;
    ackset.insert(1).await;
    ackset.insert(2).await;
    ackset.insert(6).await;

    let acks = ackset.get_ack().await;

    assert!(acks == vec![(0,2), (6,6)]);

    ackset.reset().await;

    ackset.insert(0).await;
    ackset.insert(1).await;
    ackset.insert(2).await;
    ackset.insert(5).await;
    ackset.insert(8).await;

    assert!(ackset.get_nack().await == vec![3, 4, 6, 7]);

    let acks = ackset.get_ack().await;

    assert!(acks == vec![(0,2), (5,5) , (8,8)]);

    ackset.reset().await;

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

    let a = Frame::deserialize(p.clone()).await;
    assert!(a.serialize().await == p);

}