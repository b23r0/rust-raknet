use std::sync::atomic::{AtomicU8, Ordering};

pub static ENABLE_RAKNET_LOG: AtomicU8 = AtomicU8::new(0);

/// A switch to print Raknet logs
/// 8 bit flag (00000000)
/// enable print debug log = 1[00000001]
/// enable print error log = 2[00000010]
/// enable print info log = 4[00000100]
/// enable print debug && error = 3[00000011]
pub fn enable_raknet_log(flag: u8) {
    ENABLE_RAKNET_LOG.store(flag, Ordering::Relaxed);
}

/// Print Raknet Debug Log
#[macro_export]
macro_rules! raknet_log_debug {
    ($($arg:tt)*) => ({
        if $crate::log::ENABLE_RAKNET_LOG.load(std::sync::atomic::Ordering::Relaxed) & 1 != 0 {
            let now = $crate::utils::cur_timestamp_millis();
            let msg = format!($($arg)*);
            println!("debug - {} - {} - {}" , now , "raknet" , msg);
        }
    })
}

/// Print Raknet Error Log
#[macro_export]
macro_rules! raknet_log_error {
    ($($arg:tt)*) => ({
        if $crate::log::ENABLE_RAKNET_LOG.load(std::sync::atomic::Ordering::Relaxed) & 2 != 0 {
            let now = $crate::utils::cur_timestamp_millis();
            let msg = format!($($arg)*);
            println!("error - {} - {} - {}" , now , "raknet" , msg);
        }
    })
}

/// Print Raknet Info Log
#[macro_export]
macro_rules! raknet_log_info {
    ($($arg:tt)*) => ({
        if $crate::log::ENABLE_RAKNET_LOG.load(std::sync::atomic::Ordering::Relaxed) & 4 != 0 {
            let now = $crate::utils::cur_timestamp_millis();
            let msg = format!($($arg)*);
            println!("info - {} - {} - {}" , now , "raknet" , msg);
        }
    })
}
