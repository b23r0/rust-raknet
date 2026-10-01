pub const RAKNET_PROTOCOL_VERSION: u8 = 10;
pub const RAKNET_PROTOCOL_VERSION_LIST: [u8; 2] = [10, 11];
// Keep the initial MTU below the common 1500-byte Ethernet limit.
pub const RAKNET_CLIENT_MTU: u16 = 1400;
pub(crate) const RAKNET_MAX_MTU: u16 = 1492;

pub const RECEIVE_TIMEOUT: i64 = 60000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Endian {
    Big,
    Little,
}

pub fn cur_timestamp_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(0)
}

/// Elapsed time for local deadlines; wire timestamps still use wall-clock time.
pub(crate) fn monotonic_millis() -> i64 {
    static START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    START
        .get_or_init(std::time::Instant::now)
        .elapsed()
        .as_millis()
        .min(i64::MAX as u128) as i64
}
