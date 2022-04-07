use std::sync::atomic::{AtomicBool, Ordering};

pub static ENABLE_RAKNET_LOG: AtomicBool = AtomicBool::new(false);

/// A switch to print Raknet logs
pub fn enbale_raknet_log(flag : bool){
    ENABLE_RAKNET_LOG.store(flag, Ordering::Relaxed);
}


/// Print Raknet Log
#[macro_export] macro_rules! raknet_log {
    ($($arg:tt)*) => ({
        if $crate::log::ENABLE_RAKNET_LOG.load(std::sync::atomic::Ordering::Relaxed) {
            let now = $crate::utils::cur_timestamp_millis();
            let msg = format!($($arg)*);
            println!("{} - {} - {}" , now , "raknet" , msg);
        }
    })
}