pub const RAKNET_PROTOCOL_VERSION: u8 = 10;
pub const RAKNET_PROTOCOL_VERSION_LIST: [u8; 2] = [10, 11];
//the MTU is minecraft bedrock 1.18.2 give me
pub const RAKNET_CLIENT_MTU: u16 = 1400;

pub const RECEIVE_TIMEOUT: i64 = 60000;

pub enum Endian {
    Big,
    Little,
}

pub fn cur_timestamp_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis()
        .try_into()
        .unwrap_or(0)
}

pub fn _is_timeout(time: i64, timeout: u64) -> bool {
    let cur = cur_timestamp_millis();
    cur >= time + timeout as i64
}
