use crate::{packet::*, datatype::*, utils::*};


#[derive(Clone)]
pub enum Reliability {
    Unreliable = 0x00,
    UnreliableSequenced = 0x01,
    Reliable = 0x02,
    ReliableOrdered = 0x03,
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

struct Frame{
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