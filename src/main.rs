extern crate tokio;
use params::PerfMode;

use crate::client::ClientImpl;
use crate::server::ServerImpl;
use crate::test::Test;

use std::io;
use std::process::exit;
mod client;
mod net;
mod noprotection;
mod params;
mod quic;
mod server;
mod tcp;
mod test;
mod udp;

#[tokio::main]
async fn main() -> io::Result<()> {
    let param = params::parse_args().unwrap();

    match param.mode {
        PerfMode::SERVER => loop {
            let mut test = Test::from(&param);
            let mut server = ServerImpl::new(&param)?;
            if server.run(&mut test).await? < 0 {
                exit(1);
            }
            test.reset();
        },
        PerfMode::CLIENT => {
            let test = Test::from(&param);
            let mut client = ClientImpl::new(&param).await?;
            client.run(test).await?;
            exit(0);
        }
    }
}
