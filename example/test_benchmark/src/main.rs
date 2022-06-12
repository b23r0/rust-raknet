use rust_raknet::*;
use getopts::Options;
use tokio::{net::{TcpStream, TcpListener}, io::{AsyncWriteExt, AsyncReadExt}, time::sleep};

pub fn cur_timestamp_millis() -> i64{
    std::time::SystemTime::now()
    .duration_since(std::time::UNIX_EPOCH)
    .unwrap()
    .as_millis()
    .try_into()
    .unwrap_or(0)
}

fn usage(program: &str, opts: &Options) {
    let program_path = std::path::PathBuf::from(program);
    let program_name = program_path.file_stem().unwrap().to_str().unwrap();
    let brief = format!("Usage: {} [-p] [tcp|raknet] [-t] [server|client] [-a] [IP_ADDRESS]",
                        program_name);
    print!("{}", opts.usage(&brief));
}

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().collect();
    let program = args[0].clone();

    let mut opts = Options::new();

    opts.reqopt("p",
                "protocol",
                "benchmark test protocol",
                "PROTOCOL");

	opts.reqopt("t",
                "type",
                "server or client",
                "TYPE");

	opts.reqopt("a",
                "address",
                "bind or connect connection address",
                "ADDRESS");

    let matches = opts.parse(&args[1..]).unwrap_or_else(|_| {
        usage(&program, &opts);
        std::process::exit(-1);
    });

    let proto = matches.opt_str("p").unwrap();
    let ctype = matches.opt_str("t").unwrap();
    let address = matches.opt_str("a").unwrap();

    if proto == "tcp"{
        if ctype == "client"{
            let mut client = TcpStream::connect(address).await.unwrap();
            let mut ts : Vec<i64> = Vec::new();
            let mut buf = [0u8;800];
            for _ in 0..100{
                let t1 = cur_timestamp_millis();
                client.read_exact(&mut buf).await.unwrap();
                let t = cur_timestamp_millis() - t1;
                println!("latency : {}" , t);
                ts.push(t);
            }

            let mut sum : i64 = 0;
            for i in ts.iter(){
                sum += i;
            }
            println!("avg : {}" , sum / ts.len() as i64)


        }else if ctype == "server"{
            let server = TcpListener::bind(address).await.unwrap();
            loop{
                let (mut client, _) = server.accept().await.unwrap();
                tokio::spawn(async move {
                    let mut buf = [0u8;800];
                    for _ in 0..100{
                        sleep(std::time::Duration::from_millis(30)).await;
                        client.write_all(&mut buf).await.unwrap();
                    }

                    //avoid connection closed
                    sleep(std::time::Duration::from_secs(100)).await;
                });
            }
        }
    }else if proto == "raknet" {
        if ctype == "client"{
            let mut client = RaknetSocket::connect(&address.parse().unwrap()).await.unwrap();
            let mut ts : Vec<i64> = Vec::new();
            for _ in 0..100{
                let t1 = cur_timestamp_millis();
                let _ = client.recv().await.unwrap();
                let t = cur_timestamp_millis() - t1;
                println!("latency : {}" , t);
                ts.push(t);
            }

            let mut sum : i64 = 0;
            for i in ts.iter(){
                sum += i;
            }
            println!("avg : {}" , sum / ts.len() as i64)

        }else if ctype == "server"{
            let mut server = RaknetListener::bind(&address.parse().unwrap()).await.unwrap();
            server.listen().await;
            loop{
                let mut client = server.accept().await.unwrap();
                tokio::spawn(async move {
                    let mut buf = [0xfe;800];
                    for _ in 0..100{
                        sleep(std::time::Duration::from_millis(30)).await;
                        client.send(&mut buf, Reliability::ReliableOrdered).await.unwrap();
                    }

                    //avoid connection closed
                    sleep(std::time::Duration::from_secs(100)).await;
                });
            }
        }
    }
}
