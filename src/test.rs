use crate::{params::PerfParams, stream::*};
use mio::Token;
use serde::{Deserialize, Serialize};
use serde_json;
use std::{
    fmt::Display,
    io::{self, Error},
    ops::Sub,
    time::{Duration, Instant},
};

#[derive(Clone, Copy, Debug)]
pub enum TestState {
    Start,
    ParamExchange,
    CreateStreams,
    TestStart,
    TestRunning,
    TestEnd,
    Wait,
    AccessDenied,
    End,
}

impl TestState {
    pub fn from_i8(value: i8) -> TestState {
        match value {
            0 => TestState::Start,
            1 => TestState::ParamExchange,
            2 => TestState::CreateStreams,
            3 => TestState::TestStart,
            4 => TestState::TestRunning,
            5 => TestState::TestEnd,
            6 => TestState::Wait,
            7 => TestState::AccessDenied,
            8 => TestState::End,
            _ => panic!("Unknown value: {}", value),
        }
    }
}

pub const DEFAULT_SESSION_TIMEOUT: u32 = 120;

pub struct Statistics {
    iter: u64,
    start: Instant,
    end: Instant,
    bytes: u64,
    blks: u64,
}

impl Display for Statistics {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{:>3}s {} {} bytes {} blocks",
            self.iter,
            Test::kmg(self.bytes, self.start, self.end),
            self.bytes,
            self.blks
        )
    }
}
pub struct PerfStream {
    pub stream: Box<dyn Stream>,
    pub created: Instant,
    pub start: Instant,
    pub bytes: u64,
    pub blks: u64,
    pub stats: Vec<Statistics>,
    pub curr_time: Instant,
    pub curr_bytes: u64,
    pub curr_blks: u64,
    pub curr_iter: u64,
}

impl Display for PerfStream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "[{:>3}] {:>3}s {} {} bytes {} blocks",
            self.stream.fd(),
            Instant::now().sub(self.start).as_secs(),
            Test::kmg(self.bytes, self.start, Instant::now()),
            self.bytes,
            self.blks
        )
    }
}

impl PerfStream {
    pub fn new<T: Stream + 'static>(stream: T) -> PerfStream {
        PerfStream {
            stream: Box::from(stream),
            created: Instant::now(),
            start: Instant::now(),
            bytes: 0,
            blks: 0,
            stats: Vec::new(),
            curr_time: Instant::now(),
            curr_bytes: 0,
            curr_blks: 0,
            curr_iter: 0,
        }
    }
    pub fn push_stat(&mut self) {
        let stat = Statistics {
            iter: self.curr_iter,
            start: self.curr_time,
            end: Instant::now(),
            bytes: self.curr_bytes,
            blks: self.curr_blks,
        };
        println!("[{:>3}] {}", self.stream.fd(), stat);
        self.stats.push(stat);
        // let sum: u64 = self.stats.iter().rev().take(2).map(|s| s.bytes).sum();
    }
    #[inline]
    pub fn read(&mut self) -> io::Result<usize> {
        match self.stream.read() {
            Ok(0) => {
                // println!("Zero bytes read");
                return Ok(0);
            }
            Ok(n) => {
                return Ok(n);
            }
            Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => {
                // println!("{:?}", e);
                return Err(Error::last_os_error());
            }
            Err(e) => {
                // println!("{:?}", e);
                return Err(e);
            }
        }
    }
    #[inline]
    pub fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match self.stream.write(buf) {
            Ok(n) => {
                return Ok(n);
            }
            Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => {
                return Err(Error::last_os_error());
            }
            Err(e) => {
                return Err(e.into());
            }
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Copy, Clone)]
pub enum Conn {
    TCP,
    UDP,
    QUIC,
}
impl Display for Conn {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Conn::TCP => write!(f, "TCP"),
            Conn::UDP => write!(f, "UDP"),
            Conn::QUIC => write!(f, "QUIC"),
        }
    }
}

#[derive(Serialize, Deserialize, Debug)]
struct Settings {
    conn: Conn,
    num_streams: u8,
    recv_timeout_in_secs: u32,
    mss: u32,
    bitrate: u64,
    sndbuf: usize,
    rcvbuf: usize,
    time: Duration,
    bytes: u64,
    blks: u64,
    length: u32,
}

pub struct Test {
    state: TestState,
    verbose: bool,
    debug: bool,
    skip_tls: bool,
    pub cookie: String,
    settings: Settings,
    idle_timeout_in_secs: Option<Duration>,
    reset_counter: u16,
    pub streams: Vec<PerfStream>,
    pub tokens: Vec<Token>,
    pub start: Instant,
    pub total_bytes: u64,
    pub total_blks: u64,
}

impl Test {
    pub fn new() -> Test {
        let state = TestState::Start;
        let num_streams: u8 = 1;
        let settings = Settings {
            conn: Conn::TCP,
            num_streams,
            recv_timeout_in_secs: DEFAULT_SESSION_TIMEOUT,
            mss: 0,
            bitrate: 0,
            sndbuf: 0,
            rcvbuf: 0,
            time: Duration::from_secs(10),
            bytes: 0,
            blks: 0,
            length: 0,
        };
        Test {
            state,
            settings,
            verbose: false,
            debug: false,
            skip_tls: false,
            cookie: String::new(),
            idle_timeout_in_secs: None,
            reset_counter: 0,
            streams: Vec::new(),
            tokens: Vec::new(),
            start: Instant::now(),
            total_bytes: 0,
            total_blks: 0,
        }
    }
    pub fn from(param: &PerfParams) -> Test {
        let mut test = Test::new();
        test.settings.num_streams = param.num_streams;
        test.set_idle_timeout(param.idle_timeout);
        test.verbose = param.verbose;
        test.debug = param.debug;
        test.settings.mss = param.mss;
        test.settings.bitrate = param.bitrate;
        test.settings.sndbuf = param.sendbuf as usize;
        test.settings.rcvbuf = param.recvbuf as usize;
        test.skip_tls = param.skip_tls;
        if param.udp {
            test.settings.conn = Conn::UDP;
        }
        if param.quic {
            test.settings.conn = Conn::QUIC;
        }
        test.settings.time = Duration::from_secs(param.time);
        test.settings.bytes = param.bytes;
        test.settings.blks = param.blks;
        test.settings.length = param.length;
        test
    }
    pub fn reset(&mut self) {
        self.state = TestState::Start;
        self.cookie = String::new();
        self.streams.clear();
        self.tokens.clear();
    }
    pub fn transition(&mut self, state: TestState) {
        if self.debug() {
            println!("Debug: Transition from {:?} to {:?}", self.state, state);
        }
        self.state = state;
    }
    #[inline(always)]
    pub fn state(&self) -> TestState {
        self.state
    }
    pub fn num_streams(&self) -> u8 {
        return self.settings.num_streams;
    }
    pub fn mss(&self) -> u32 {
        return self.settings.mss;
    }
    pub fn sndbuf(&self) -> u32 {
        return self.settings.sndbuf as u32;
    }
    pub fn rcvbuf(&self) -> u32 {
        return self.settings.rcvbuf as u32;
    }
    pub fn verbose(&self) -> bool {
        return self.verbose;
    }
    pub fn debug(&self) -> bool {
        return self.debug;
    }
    pub fn skip_tls(&self) -> bool {
        return self.skip_tls;
    }
    /*
    pub fn recv_timeout(&self) -> u32 {
        return self.settings.recv_timeout_in_secs;
    }
    */
    pub fn set_settings(&mut self, settings: String) {
        self.settings = serde_json::from_str(&settings).unwrap();
    }
    pub fn settings(&self) -> String {
        return serde_json::to_string(&self.settings).unwrap();
    }
    #[inline(always)]
    pub fn conn(&self) -> Conn {
        self.settings.conn
    }
    #[inline(always)]
    pub fn time(&self) -> Duration {
        self.settings.time
    }
    #[inline(always)]
    pub fn bytes(&self) -> u64 {
        self.settings.bytes
    }
    #[inline(always)]
    pub fn blks(&self) -> u64 {
        self.settings.blks
    }
    pub fn length(&self) -> u32 {
        self.settings.length
    }
    pub fn set_idle_timeout(&mut self, idle_timeout: u32) {
        if idle_timeout > 0 {
            self.idle_timeout_in_secs = Some(Duration::from_secs(idle_timeout as u64));
        }
    }
    pub fn idle_timeout(&self) -> Option<Duration> {
        return self.idle_timeout_in_secs;
    }
    pub fn reset_counter_inc(&mut self) -> u16 {
        self.reset_counter += 1;
        return self.reset_counter;
    }
    pub fn print_stats(&self) {
        println!("- - - - - - - - - - - - - - - - - - - - - - - - - - - -");
        for pstream in &self.streams {
            println!("{}", pstream);
        }
        if self.streams.len() > 1 {
            let bytes: u64 = self.streams.iter().map(|s| s.bytes).sum();
            let blks: u64 = self.streams.iter().map(|s| s.blks).sum();
            println!(
                "[Sum] {:>3}s {} {} bytes {} blocks",
                Instant::now().sub(self.start).as_secs(),
                Test::kmg(bytes, self.start, Instant::now()),
                bytes,
                blks
            );
        }
    }
    pub fn kmg(bytes: u64, start: Instant, end: Instant) -> String {
        if bytes >= 1024 * 1024 * 1024 {
            let b: f64 = bytes as f64 / (1024 * 1024 * 1024) as f64;
            let r = (b * 8 as f64) / end.sub(start).as_secs_f64();
            return format!("{:.2} GBytes {:.1} Gbits/sec", b, r).to_string();
        } else if bytes >= 1024 * 1024 {
            let b: f64 = bytes as f64 / (1024 * 1024) as f64;
            let r = (b * 8 as f64) / end.sub(start).as_secs_f64();
            return format!("{:.2} MBytes {:.1} Mbits/sec", b, r).to_string();
        } else if bytes >= 1024 {
            let b: f64 = bytes as f64 / 1024 as f64;
            let r = (b * 8 as f64) / end.sub(start).as_secs_f64();
            return format!("{:.2} KBytes {:.1} Kbits/sec", b, r).to_string();
        } else {
            let r = (bytes as f64 * 8 as f64) / end.sub(start).as_secs_f64();
            return format!("{} Bytes {:.1} bits/sec", bytes, r).to_string();
        }
    }
}
