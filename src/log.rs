use std::sync::atomic::{AtomicU8, Ordering};

pub static ENABLE_RAKNET_LOG: AtomicU8 = AtomicU8::new(0);

/// Sets the log categories to print.
///
/// Bit 0 enables debug logs, bit 1 enables errors, and bit 2 enables info logs.
pub fn enable_raknet_log(flag: u8) {
    ENABLE_RAKNET_LOG.store(flag, Ordering::Relaxed);
}

/// Print a debug log when debug logging is enabled.
#[macro_export]
macro_rules! raknet_log_debug {
    ($($arg:tt)*) => ({
        if $crate::log::ENABLE_RAKNET_LOG.load(std::sync::atomic::Ordering::Relaxed) & 1 != 0 {
            let now = $crate::utils::cur_timestamp_millis();
            println!("debug - {now} - raknet - {}", format_args!($($arg)*));
        }
    })
}

/// Print an error log when error logging is enabled.
#[macro_export]
macro_rules! raknet_log_error {
    ($($arg:tt)*) => ({
        if $crate::log::ENABLE_RAKNET_LOG.load(std::sync::atomic::Ordering::Relaxed) & 2 != 0 {
            let now = $crate::utils::cur_timestamp_millis();
            eprintln!("error - {now} - raknet - {}", format_args!($($arg)*));
        }
    })
}

/// Print an info log when info logging is enabled.
#[macro_export]
macro_rules! raknet_log_info {
    ($($arg:tt)*) => ({
        if $crate::log::ENABLE_RAKNET_LOG.load(std::sync::atomic::Ordering::Relaxed) & 4 != 0 {
            let now = $crate::utils::cur_timestamp_millis();
            println!("info - {now} - raknet - {}", format_args!($($arg)*));
        }
    })
}
