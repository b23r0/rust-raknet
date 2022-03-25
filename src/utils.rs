use chrono::prelude::*;

pub const RAKNET_PROTOCOL_VERSION : u8 = 10;
//the MTU is minecraft bedrock 1.18.2 give me
pub const RAKNET_CLIENT_MTU : u16 = 1400;

pub enum Endian {
    Big,
    Little,
}


pub fn cur_timestamp() -> i64{
    let dt = Local::now();
    dt.timestamp()
}

pub fn cur_timestamp_millis() -> i64{
    let dt = Local::now();
    dt.timestamp_millis()
}

pub fn _is_timeout(time : i64, timeout : u64) -> bool{
    let cur = cur_timestamp();
    cur >= time + timeout as i64
}