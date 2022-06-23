use getopts::Options;
use rust_raknet::*;

fn usage(program: &str, opts: &Options) {
    let program_path = std::path::PathBuf::from(program);
    let program_name = program_path.file_stem().unwrap().to_str().unwrap();
    let brief = format!("Usage: {} [-l] [LOCAL_ADDR] [-r] [REMOTE_ADDRESS]",
                        program_name);
    print!("{}", opts.usage(&brief));
}

#[tokio::main]
async fn main() {

    let args: Vec<String> = std::env::args().collect();
    let program = args[0].clone();

    let mut opts = Options::new();

    opts.reqopt("l",
                "local_address",
                "The address on which to listen raknet proxy for incoming requests",
                "LISTEN_ADDR");

	opts.reqopt("r",
                "remote_address",
                "The address is remote raknet server",
                "REMOTE_ADDRESS");

    let matches = opts.parse(&args[1..]).unwrap_or_else(|_| {
        usage(&program, &opts);
        std::process::exit(-1);
    });

    let local_address : String = match match matches.opt_str("l"){
        Some(p) => p,
        None => {
            return;
        },
    }.parse(){
        Err(_) => {
            return;
        },
        Ok(p) => p
    };

    let remote_address : String = match match matches.opt_str("r"){
        Some(p) => p,
        None => {
            return;
        },
    }.parse(){
        Err(_) => {
            return;
        },
        Ok(p) => p
    };

    let mut listener = RaknetListener::bind(&local_address.parse().unwrap()).await.unwrap();
    listener.listen().await;
    loop{
        let mut client1 = listener.accept().await.unwrap();
        let remote_address = remote_address.clone();
        tokio::spawn(async move {
            let mut client2 = match RaknetSocket::connect(&remote_address.parse().unwrap()).await{
                Ok(p) => p,
                Err(e) => {
                    println!("connect remote raknet server faild : {:?}", e);
                    client1.close().await.unwrap();
                    return;
                },
            };
            println!("build connection");
            loop{
                tokio::select!{
                    a = client1.recv() => {
                        let a = match a{
                            Ok(p) => p,
                            Err(_) => {
                                client2.close().await.unwrap();
                                break;
                            },
                        };
                        match client2.send(&a, Reliability::ReliableOrdered).await{
                            Ok(p) => p,
                            Err(_) => {
                                client1.close().await.unwrap();
                                break;
                            },
                        };
                    },
                    b = client2.recv() => {
                        let b = match b{
                            Ok(p) => p,
                            Err(_) => {
                                client1.close().await.unwrap();
                                break;
                            },
                        };
                        match client1.send(&b, Reliability::ReliableOrdered).await{
                            Ok(p) => p,
                            Err(_) => {
                                client2.close().await.unwrap();
                                break;
                            },
                        };
                    }
                }
            }
            client1.close().await.unwrap();
            client2.close().await.unwrap();
            println!("close connection");
        });
    };
}
