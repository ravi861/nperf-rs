use std::{fmt::Display, io};

extern crate argparse;
use argparse::{ArgumentParser, Store, StoreTrue};

use crate::test::DEFAULT_SESSION_TIMEOUT;

#[derive(Clone, Copy)]
pub enum PerfMode {
    SERVER,
    CLIENT,
}

impl Display for PerfMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PerfMode::SERVER => write!(f, "server"),
            PerfMode::CLIENT => write!(f, "client"),
        }
    }
}

pub struct PerfParams {
    pub mode: PerfMode,
    pub udp: bool,
    pub quic: bool,
    pub verbose: bool,
    pub debug: bool,
    pub bindaddr: String,
    pub port: u16,
    pub dev: String,
    pub recv_timeout: u32,
    pub idle_timeout: u32,
    pub num_streams: u8,
    pub mss: u32,
    pub sendbuf: u32,
    pub recvbuf: u32,
    pub skip_tls: bool,
    pub time: u64,
    pub bytes: u64,
    pub blks: u64,
    pub length: u32,
}

pub fn parse_args() -> Result<PerfParams, io::ErrorKind> {
    let mut server = false;
    let mut client = false;
    let mut port: u16 = 8080;
    let mut udp: bool = false;
    let mut quic: bool = false;
    let mut dev: String = String::new();
    let mut bindaddr: String = String::from("127.0.0.1");
    let mut recv_timeout: u32 = DEFAULT_SESSION_TIMEOUT;
    let mut idle_timeout: u32 = 0;
    let mut num_streams: u8 = 1;
    let mut mss: u32 = 0;
    let mut sendbuf: u32 = 0;
    let mut recvbuf: u32 = 0;
    let mut skip_tls: bool = false;
    let mut verbose = false;
    let mut debug = false;
    let mut time: u64 = 10;
    let mut bytes: u64 = 0;
    let mut blks: u64 = 0;
    let mut length: u32 = 0;
    {
        let mut args = ArgumentParser::new();
        args.set_description("Greet somebody.");
        args.refer(&mut server).add_option(
            &["-s", "--server"],
            StoreTrue,
            "[s] Start perf as server",
        );
        args.refer(&mut client).add_option(
            &["-c", "--client"],
            StoreTrue,
            "[c] Start perf as client",
        );
        args.refer(&mut udp)
            .add_option(&["-u", "--udp"], StoreTrue, "[c] Use UDP");
        args.refer(&mut quic)
            .add_option(&["-q", "--quic"], StoreTrue, "[c] Use QUIC");
        args.refer(&mut port)
            .add_option(&["-p", "--port"], Store, "[s] Port server listen on");
        args.refer(&mut bindaddr).add_option(
            &["-B", "--bind-addr"],
            Store,
            "Bind address to listen on",
        );
        args.refer(&mut dev)
            .add_option(&["-b", "--bind-dev"], Store, "Bind to device");
        args.refer(&mut recv_timeout).add_option(
            &["--recv-timeout"],
            Store,
            "[sc] idle timeout for receiving data (default 120s)",
        );
        args.refer(&mut time).add_option(
            &["-t", "--time"],
            Store,
            "[c] time in seconds to transmit for (default 10 secs)",
        );
        args.refer(&mut bytes).add_option(
            &["-n", "--bytes"],
            Store,
            "[c] number of bytes to transmit (instead of -t)",
        );
        args.refer(&mut blks).add_option(
            &["-k", "--blocks"],
            Store,
            "[c] number of blocks (packets) to transmit (instead of -t or -n)",
        );
        args.refer(&mut length).add_option(
            &["-l", "--length"],
            Store,
            "[c] length of buffer to read or write (default 128 KB for TCP, dynamic or 1460 for UDP)",
        );
        args.refer(&mut idle_timeout).add_option(
            &["--idle-timeout"],
            Store,
            "[s] restart idle server after # seconds in case it got stuck (default - no timeout)",
        );
        args.refer(&mut num_streams).add_option(
            &["-P", "--parallel"],
            Store,
            "[c] number of parallel client streams to run",
        );
        args.refer(&mut mss).add_option(
            &["-M", "--set-mss"],
            Store,
            "[c] set TCP maximum segment size",
        );
        args.refer(&mut sendbuf).add_option(
            &["--send-buf-size"],
            Store,
            "[c] set socket send buffer size [default: OS defined]",
        );
        args.refer(&mut recvbuf).add_option(
            &["--recv-buf-size"],
            Store,
            "[c] set socket receive buffer size [default: OS defined]",
        );
        args.refer(&mut skip_tls).add_option(
            &["--skip-tls"],
            StoreTrue,
            "[c] Disable QUIC connection encryption",
        );
        args.refer(&mut verbose).add_option(
            &["-V", "--verbose"],
            StoreTrue,
            "[sc] Enable verbose logging",
        );
        args.refer(&mut debug).add_option(
            &["-d", "--debug"],
            StoreTrue,
            "[sc] Enable debug logging",
        );
        args.parse_args_or_exit();
    }
    if server && client {
        return Err(io::ErrorKind::InvalidInput);
    }
    let mut mode: PerfMode = PerfMode::SERVER;
    if client {
        mode = PerfMode::CLIENT;
    }
    if server {
        mode = PerfMode::SERVER;
    }
    let params = PerfParams {
        mode,
        udp,
        quic,
        verbose,
        debug,
        bindaddr,
        port,
        dev,
        recv_timeout,
        idle_timeout,
        num_streams,
        mss,
        sendbuf,
        recvbuf,
        skip_tls,
        time,
        bytes,
        blks,
        length,
    };
    Ok(params)
}
