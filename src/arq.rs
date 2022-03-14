use crate::{packet::*, datatype::*};


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