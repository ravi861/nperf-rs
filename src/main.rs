use futures::executor::block_on;
use std::io;
use std::process::exit;

use crate::client::ClientImpl;
use crate::params::PerfMode;
use crate::server::ServerImpl;
use crate::test::Test;

mod client;
mod net;
mod noprotection;
mod params;
mod quic;
mod server;
mod tcp;
mod test;
mod udp;

fn main() -> io::Result<()> {
    let param = params::parse_args().unwrap();

    match param.mode {
        PerfMode::SERVER => loop {
            let mut test = Test::from(&param);
            let mut server = ServerImpl::new(&param)?;
            let run = server.run(&mut test);
            if block_on(run)? < 0 {
                exit(1);
            }
            test.reset();
        },
        PerfMode::CLIENT => {
            let test = Test::from(&param);
            let mut client = ClientImpl::new(&param)?;
            let run = client.run(test);
            block_on(run)?;
            exit(0);
        }
    }
}
