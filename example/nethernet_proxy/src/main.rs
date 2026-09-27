//! Start the library's NetherNet signaling forwarder.

use rust_raknet::NetherNetProxy;
use std::{env, net::SocketAddr, process};

fn usage(program: &str) -> ! {
    eprintln!(
        "Usage: {program} --listen <IP:PORT> --upstream <IP:PORT>\n\
         Example: {program} --listen 127.0.0.1:19144 --upstream 127.0.0.1:19142\n\
         For NetherNet signaling TCP only. WebRTC gameplay uses direct UDP."
    );
    process::exit(2);
}

fn parse_args() -> (SocketAddr, SocketAddr) {
    let mut args = env::args();
    let program = args
        .next()
        .unwrap_or_else(|| "nethernet-signaling-proxy".into());
    let mut listen = None;
    let mut upstream = None;
    while let Some(arg) = args.next() {
        let value = args.next().unwrap_or_else(|| usage(&program));
        match arg.as_str() {
            "--listen" => listen = Some(value.parse().unwrap_or_else(|_| usage(&program))),
            "--upstream" => upstream = Some(value.parse().unwrap_or_else(|_| usage(&program))),
            _ => usage(&program),
        }
    }
    match (listen, upstream) {
        (Some(listen), Some(upstream)) => (listen, upstream),
        _ => usage(&program),
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let (listen, upstream) = parse_args();
    let proxy = NetherNetProxy::bind(listen, upstream).await?;
    println!(
        "NetherNet signaling TCP {} → {}; WebRTC game traffic connects directly to the server",
        proxy.local_addr()?,
        upstream
    );
    proxy.run().await?;
    Ok(())
}
