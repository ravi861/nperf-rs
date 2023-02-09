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
    pub verbose: bool,
    pub debug: bool,
    pub bindaddr: String,
    pub port: u16,
    pub dev: String,
    pub recv_timeout: u32,
    pub idle_timeout: u32,
    pub num_streams: u8,
    pub mss: u32,
}

pub fn parse_args() -> Result<PerfParams, io::ErrorKind> {
    let mut server = false;
    let mut client = false;
    let mut port: u16 = 8080;
    let mut udp: bool = false;
    let mut dev: String = String::new();
    let mut bindaddr: String = String::from("127.0.0.1");
    let mut recv_timeout: u32 = DEFAULT_SESSION_TIMEOUT;
    let mut idle_timeout: u32 = 0;
    let mut num_streams: u8 = 1;
    let mut verbose = false;
    let mut debug = false;
    let mut mss: u32 = 0;
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
        args.refer(&mut mss).add_option(
            &["-M", "--set-mss"],
            Store,
            "[c] set TCP maximum segment size",
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
        verbose,
        debug,
        bindaddr,
        port,
        dev,
        recv_timeout,
        idle_timeout,
        num_streams,
        mss,
    };
    Ok(params)
}
