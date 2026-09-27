//! Start the library's NetherNet signaling forwarder.

use rust_raknet::NetherNetProxy;
use std::{env, net::SocketAddr, process};

fn usage(program: &str) -> ! {
    eprintln!(
        "Usage: {program} --listen <IP:PORT> --upstream <IP:PORT>\n\
         Example: {program} --listen 127.0.0.1:19144 --upstream 127.0.0.1:19142\n\
         This forwards NetherNet signaling over TCP; WebRTC gameplay uses direct UDP."
    );
    process::exit(2);
}

fn parse_address(option: &str, value: &str) -> Result<SocketAddr, String> {
    value
        .parse()
        .map_err(|error| format!("invalid address for {option}: {error}"))
}

fn parse_args() -> Result<(SocketAddr, SocketAddr), String> {
    let mut args = env::args().skip(1);
    let mut listen = None;
    let mut upstream = None;

    while let Some(option) = args.next() {
        let value = args
            .next()
            .ok_or_else(|| format!("missing address after {option}"))?;
        match option.as_str() {
            "--listen" => {
                if listen.replace(parse_address(&option, &value)?).is_some() {
                    return Err("--listen may only be specified once".to_owned());
                }
            }
            "--upstream" => {
                if upstream.replace(parse_address(&option, &value)?).is_some() {
                    return Err("--upstream may only be specified once".to_owned());
                }
            }
            _ => return Err(format!("unknown option: {option}")),
        }
    }

    let listen = listen.ok_or_else(|| "--listen is required".to_owned())?;
    let upstream = upstream.ok_or_else(|| "--upstream is required".to_owned())?;
    Ok((listen, upstream))
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let program = env::args()
        .next()
        .unwrap_or_else(|| "nethernet-signaling-proxy".to_owned());
    let (listen, upstream) = parse_args().unwrap_or_else(|error| {
        eprintln!("{error}");
        usage(&program)
    });

    let proxy = NetherNetProxy::bind(listen, upstream).await?;
    println!(
        "NetherNet signaling TCP {} → {}; WebRTC game traffic connects directly to the server",
        proxy.local_addr()?,
        upstream
    );
    proxy.run().await?;
    Ok(())
}
