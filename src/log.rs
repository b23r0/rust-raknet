use std::sync::atomic::{AtomicBool, Ordering};

pub static ENABLE_RAKNET_LOG: AtomicBool = AtomicBool::new(false);

pub fn enbale_raknet_log(flag : bool){
    ENABLE_RAKNET_LOG.fetch_and(flag, Ordering::Relaxed);
}

#[macro_export] macro_rules! raknet_log {
    ($($arg:tt)*) => ({
        if $crate::log::ENABLE_RAKNET_LOG.load(std::sync::atomic::Ordering::Relaxed) {
            let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis()
            .try_into()
            .unwrap_or(0);

            print!("{} - " , now);
            print!("raknet - ");
            print!($($arg)*);
            print!("\n");
        }
    })
}