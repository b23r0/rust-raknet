use chrono::prelude::*;

pub const RAKNET_PROTOCOL_VERSION : u8 = 10;
pub const RAKNET_MTU : u16 = 1500;

pub enum Endian {
    Big,
    Little,
}


pub fn cur_timestamp() -> i64{
    let dt = Local::now();
    dt.timestamp()
}

pub fn is_timeout(time : i64, timeout : u64) -> bool{
    let cur = cur_timestamp();
    cur >= time + timeout as i64
}