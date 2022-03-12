mod socket;
mod packet;
mod utils;
mod reader;
mod writer;
pub mod server;
pub use crate::server::*;

#[tokio::test]
async fn test_server_main(){
    let mut l = RaknetListener::bind("0.0.0.0:19132".parse().unwrap()).await.unwrap();
    let a = l.accept().await.unwrap();
}