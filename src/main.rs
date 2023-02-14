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
mod stream;
mod test;

#[tokio::main]
async fn main() -> io::Result<()> {
    let param = params::parse_args().unwrap();
    let mut test = Test::new();
    test.set_num_streams(param.num_streams);
    test.set_idle_timeout(param.idle_timeout);
    test.set_verbose(param.verbose);
    test.set_debug(param.debug);
    test.set_mss(param.mss);
    test.set_sndbuf(param.sendbuf);
    test.set_rcvbuf(param.recvbuf);
    if param.udp {
        test.set_udp();
    }
    if param.quic {
        test.set_quic();
    }

    match param.mode {
        PerfMode::SERVER => loop {
            let mut server = ServerImpl::new(&param)?;
            if server.run(&mut test).await? < 0 {
                exit(1);
            }
            test.reset();
        },
        PerfMode::CLIENT => {
            let mut client = ClientImpl::new(&param)?;
            client.run(test).await?;
        }
    }
    Ok(())
}
