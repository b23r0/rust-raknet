use chrono::prelude::*;

pub fn cur_timestamp() -> i64{
    let dt = Local::now();
    dt.timestamp()
}

pub fn is_timeout(time : i64, timeout : u64) -> bool{
    let cur = cur_timestamp();
    cur >= time + timeout as i64
}