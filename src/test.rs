use crate::params::PerfParams;
use mio::{Poll, Token};
use serde::{Deserialize, Serialize};
use serde_json;
use std::{
    any::Any,
    fmt::Display,
    io::{self},
    ops::Sub,
    os::unix::prelude::RawFd,
    time::{Duration, Instant},
};

struct Dummy {}
impl Stream for Dummy {
    #[inline(always)]
    fn read(&mut self, _buf: &mut [u8]) -> io::Result<usize> {
        Ok(0)
    }
    #[inline(always)]
    fn write(&mut self, _buf: &[u8]) -> io::Result<usize> {
        Ok(0)
    }
    fn fd(&self) -> RawFd {
        0
    }
    fn register(&mut self, _poll: &mut Poll, _token: Token) {}
    fn deregister(&mut self, _poll: &mut Poll) {}
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
    fn print_new_stream(&self) {}
    fn socket_type(&self) -> Conn {
        Conn::TCP
    }
}

impl Default for Box<dyn Stream> {
    fn default() -> Box<(dyn Stream + 'static)> {
        Box::new(Dummy {})
    }
}

pub const DEFAULT_SESSION_TIMEOUT: u32 = 120;
pub const MAX_TCP_PAYLOAD: usize = 131071;
pub const MAX_UDP_PAYLOAD: usize = 65500;
pub const MAX_QUIC_PAYLOAD: usize = 65500;

pub const ONE_SEC: Duration = Duration::from_secs(1);
pub const ONE_GB: u64 = 1024 * 1024 * 1024;
pub const ONE_MB: u64 = 1024 * 1024;
pub const ONE_KB: u64 = 1024;

pub trait Stream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize>;
    fn write(&mut self, buf: &[u8]) -> io::Result<usize>;
    fn fd(&self) -> RawFd;
    fn as_any(&self) -> &dyn Any;
    fn as_any_mut(&mut self) -> &mut dyn Any;
    fn register(&mut self, poll: &mut Poll, token: Token);
    fn deregister(&mut self, poll: &mut Poll);
    fn print_new_stream(&self);
    fn socket_type(&self) -> Conn;
}

#[derive(Clone, Copy, Debug)]
pub enum TestState {
    Start,
    ParamExchange,
    CreateStreams,
    TestStart,
    TestRunning,
    TestEnd,
    Wait,
    ExchangeResults,
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
            7 => TestState::ExchangeResults,
            8 => TestState::End,
            _ => panic!("Unknown value: {}", value),
        }
    }
}

impl Default for TestState {
    fn default() -> Self {
        TestState::Start
    }
}

#[derive(Serialize, Deserialize, Debug, Copy, Clone, PartialEq)]
pub enum StreamMode {
    SENDER,
    RECEIVER,
    BOTH,
}

impl Display for StreamMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StreamMode::SENDER => write!(f, "sender"),
            StreamMode::RECEIVER => write!(f, "receiver"),
            StreamMode::BOTH => write!(f, "both"),
        }
    }
}

pub struct Timers {
    pub created: Instant,
    pub start: Instant,
    pub end: Instant,
    pub curr: Instant,
}
impl Default for Timers {
    fn default() -> Self {
        Timers {
            created: Instant::now(),
            start: Instant::now(),
            end: Instant::now(),
            curr: Instant::now(),
        }
    }
}

#[derive(Serialize, Deserialize)]
pub struct StreamData {
    pub bytes: u64,
    pub blks: u64,
    pub lost: u64,
}
impl Default for StreamData {
    fn default() -> Self {
        StreamData {
            bytes: 0,
            blks: 0,
            lost: 0,
        }
    }
}

// Per second statistics
pub struct IntervalStats {
    pub iter: u64,
    timers: Timers,
    data: StreamData,
}

impl Display for IntervalStats {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{:>4}s  {}",
            self.iter,
            Test::kmg(
                self.data.bytes,
                self.timers.end.sub(self.timers.start).as_secs_f64()
            ),
        )
    }
}

#[derive(Serialize, Deserialize)]
pub struct PerfStream {
    pub mode: StreamMode,
    #[serde(skip)]
    pub stream: Box<dyn Stream>,
    #[serde(skip)]
    pub timers: Timers,
    pub elapsed: f64,
    pub data: StreamData,
    #[serde(skip)]
    pub first_recvd_in_interval: u64,
    #[serde(skip)]
    pub last_recvd_in_interval: u64,
    #[serde(skip)]
    pub stats: Vec<IntervalStats>,
    #[serde(skip)]
    pub iter: u64,
    #[serde(skip)]
    pub temp: StreamData,
}

impl Display for PerfStream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let out = match self.mode {
            StreamMode::SENDER => format!("{}", self.data.blks),
            StreamMode::RECEIVER => match self.stream.as_ref().socket_type() {
                Conn::UDP => format!("{}/{}", self.data.lost, self.data.blks),
                Conn::TCP => format!("{}", self.data.blks),
                Conn::QUIC => format!("{}", self.data.blks),
            },
            StreamMode::BOTH => format!(""),
        };
        write!(
            f,
            "[{:>3}] {:>4}s  {}  {}  {}",
            self.stream.fd(),
            self.elapsed as u64,
            Test::kmg(self.data.bytes, self.elapsed),
            out,
            self.mode
        )
    }
}

impl PerfStream {
    pub fn new<T: Stream + 'static>(stream: T, mode: StreamMode) -> PerfStream {
        PerfStream {
            mode,
            stream: Box::from(stream),
            timers: Timers::default(),
            elapsed: 0.0,
            data: StreamData::default(),
            first_recvd_in_interval: 0,
            last_recvd_in_interval: 0,
            stats: Vec::new(),
            iter: 0,
            temp: StreamData::default(),
        }
    }
    pub fn push_stat(&mut self, debug: bool) {
        let mut stat = IntervalStats {
            timers: Timers::default(),
            iter: self.iter,
            data: StreamData {
                bytes: self.temp.bytes,
                blks: self.temp.blks,
                lost: 0,
            },
        };
        match self.stream.as_ref().socket_type() {
            Conn::UDP => {
                stat.data.lost =
                    self.last_recvd_in_interval - self.first_recvd_in_interval - self.temp.blks;
                self.first_recvd_in_interval = self.last_recvd_in_interval;
            }
            _ => {}
        }
        stat.timers.start = self.timers.curr;

        let out = match self.mode {
            StreamMode::SENDER => format!("{}", stat.data.blks),
            StreamMode::RECEIVER => match self.stream.as_ref().socket_type() {
                Conn::UDP => {
                    format!("{}/{}", stat.data.lost, stat.data.blks)
                }
                Conn::TCP => format!("{}", stat.data.blks),
                Conn::QUIC => format!("{}", stat.data.blks),
            },
            StreamMode::BOTH => String::new(),
        };
        println!("[{:>3}] {}  {}", self.stream.fd(), stat, out);
        if debug {
            println!(
                "interval {} secs, {} bytes",
                stat.timers.start.elapsed().as_secs_f64(),
                stat.data.bytes
            )
        }

        self.stats.push(stat);

        // reset temp variables for next interval
        self.iter += 1;
        self.timers.curr = Instant::now();
        self.temp.bytes = 0;
        self.temp.blks = 0;
    }
    #[inline]
    pub fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self.stream.read(buf) {
            Ok(0) => {
                // println!("Zero bytes read");
                return Ok(0);
            }
            Ok(n) => {
                return Ok(n);
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
            Err(e) => {
                // println!("In write {:?}", e);
                return Err(e);
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
    length: usize,
}
impl Default for Settings {
    fn default() -> Self {
        Settings {
            conn: Conn::TCP,
            num_streams: 1,
            recv_timeout_in_secs: DEFAULT_SESSION_TIMEOUT,
            mss: 0,
            bitrate: 0,
            sndbuf: 0,
            rcvbuf: 0,
            time: Duration::from_secs(10),
            bytes: 0,
            blks: 0,
            length: MAX_TCP_PAYLOAD,
        }
    }
}

#[derive(Serialize, Deserialize)]
pub struct Test {
    pub mode: StreamMode,
    #[serde(skip)]
    state: TestState,
    #[serde(skip)]
    verbose: bool,
    #[serde(skip)]
    pub debug: bool,
    #[serde(skip)]
    skip_tls: bool,
    #[serde(skip)]
    pub cookie: String,
    #[serde(skip)]
    pub cookie_count: u8,
    #[serde(skip)]
    settings: Settings,
    #[serde(skip)]
    idle_timeout_in_secs: Option<Duration>,
    #[serde(skip)]
    reset_counter: u16,
    pub streams: Vec<PerfStream>,
    #[serde(skip)]
    pub peer_streams: Vec<PerfStream>,
    #[serde(skip)]
    pub tokens: Vec<Token>,
    #[serde(skip)]
    pub timers: Timers,
    pub elapsed: f64,
    pub data: StreamData,
    #[serde(skip)]
    peer_elapsed: f64,
    #[serde(skip)]
    peer: StreamData,
    peer_mode: StreamMode,
}

impl Test {
    pub fn new() -> Test {
        Test {
            mode: StreamMode::RECEIVER,
            state: TestState::default(),
            settings: Settings::default(),
            verbose: false,
            debug: false,
            skip_tls: false,
            cookie: String::new(),
            cookie_count: 0,
            idle_timeout_in_secs: None,
            reset_counter: 0,
            streams: Vec::new(),
            peer_streams: Vec::new(),
            tokens: Vec::new(),
            timers: Timers::default(),
            elapsed: 0.0,
            data: StreamData::default(),
            peer_elapsed: 0.0,
            peer: StreamData::default(),
            peer_mode: StreamMode::SENDER,
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
            test.settings.length = MAX_UDP_PAYLOAD;
        }
        if param.quic {
            test.settings.conn = Conn::QUIC;
            test.settings.length = MAX_QUIC_PAYLOAD;
        }
        test.settings.time = Duration::from_secs(param.time);
        if param.bytes > 0 {
            test.settings.bytes = param.bytes;
            test.settings.time = Duration::from_secs(3600);
        }
        if param.blks > 0 {
            test.settings.blks = param.blks;
            test.settings.time = Duration::from_secs(3600);
        }
        if param.length > 0 {
            test.settings.length = param.length as usize;
        }
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
    pub fn from_serde(&mut self, json: String) {
        let t: Test = serde_json::from_str(&json).unwrap();
        self.peer_streams = t.streams;
        self.peer_elapsed = t.elapsed;
        self.peer.bytes = t.data.bytes;
        self.peer.blks = t.data.blks;
        self.peer.lost = t.data.lost;
        self.peer_mode = t.mode;
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
    pub fn _sndbuf(&self) -> u32 {
        return self.settings.sndbuf as u32;
    }
    pub fn _rcvbuf(&self) -> u32 {
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
    pub fn mode(&self) -> StreamMode {
        self.mode
    }
    #[inline(always)]
    pub fn bitrate(&self) -> u64 {
        self.settings.bitrate
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
    #[inline(always)]
    pub fn length(&self) -> usize {
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
    pub fn start(&mut self) {
        for pstream in &mut self.streams {
            pstream.timers.curr = Instant::now();
        }
        self.timers.start = Instant::now();
    }
    pub fn end(&mut self, poll: &mut Poll) {
        for pstream in &mut self.streams {
            if pstream.temp.bytes > 0 {
                pstream.push_stat(self.debug);
            }
            pstream.timers.end = Instant::now();
            pstream.elapsed = pstream.timers.start.elapsed().as_secs_f64();
            pstream.stream.deregister(poll);
        }
        match self.conn() {
            Conn::UDP => {
                for pstream in &mut self.streams {
                    pstream.data.lost = pstream.stats.iter().map(|s| s.data.lost).sum();
                }
                self.data.lost = self.streams.iter().map(|s| s.data.lost).sum();
            }
            _ => {}
        }
        self.elapsed = self.timers.start.elapsed().as_secs_f64();
    }
    pub fn results(&self) -> String {
        serde_json::to_string(&self).unwrap()
    }
    pub fn print_stats(&self) {
        println!("- - - - - - - - - - - - - - Results - - - - - - - - - - - - - - - -");
        for pstream in &self.streams {
            println!("{}", pstream);
            if self.debug {
                println!(
                    "interval {} secs, {} bytes",
                    pstream.elapsed, pstream.data.bytes
                )
            }
        }
        for pstream in &self.peer_streams {
            println!("{}", pstream);
            if self.debug {
                println!(
                    "interval {} secs, {} bytes",
                    pstream.elapsed, pstream.data.bytes
                )
            }
        }
        if self.streams.len() > 1 {
            let out = match self.mode {
                StreamMode::SENDER => format!("{}", self.data.blks),
                StreamMode::RECEIVER => match self.conn() {
                    Conn::UDP => {
                        format!("{}/{}", self.data.lost, self.data.blks)
                    }
                    Conn::TCP => format!("{}", self.data.blks),
                    Conn::QUIC => format!("{}", self.data.blks),
                },
                StreamMode::BOTH => String::new(),
            };
            println!(
                "[Sum]  {:>3}s  {}  {}  {}",
                self.elapsed as u64,
                Test::kmg(self.data.bytes, self.elapsed),
                out,
                self.mode,
            );
            if self.debug {
                println!("interval {} secs, {} bytes", self.elapsed, self.data.bytes);
            }
            if self.mode == StreamMode::RECEIVER {
                println!("");
                return;
            }
            let out = match self.peer_mode {
                StreamMode::SENDER => format!("{}", self.peer.blks),
                StreamMode::RECEIVER => match self.conn() {
                    Conn::UDP => {
                        format!("{}/{}", self.peer.lost, self.peer.blks)
                    }
                    Conn::TCP => format!("{}", self.peer.blks),
                    Conn::QUIC => format!("{}", self.peer.blks),
                },
                StreamMode::BOTH => String::new(),
            };
            println!(
                "[Sum]  {:>3}s  {}  {}  {}",
                self.peer_elapsed as u64,
                Test::kmg(self.peer.bytes, self.peer_elapsed),
                out,
                self.peer_mode,
            );
            if self.debug {
                println!(
                    "interval {} secs, {} bytes",
                    self.peer_elapsed, self.peer.bytes
                );
            }
        }
        println!("");
    }
    pub fn kmg(bytes: u64, dur: f64) -> String {
        if bytes >= ONE_GB {
            let b: f64 = bytes as f64 / ONE_GB as f64;
            let r = (b * 8 as f64) / dur;
            return format!("{:<6.2} GBytes   {:<5.1} Gbits/sec", b, r).to_string();
        } else if bytes >= ONE_MB {
            let b: f64 = bytes as f64 / ONE_MB as f64;
            let r = (b * 8 as f64) / dur;
            return format!("{:<6.2} MBytes   {:<5.1} Mbits/sec", b, r).to_string();
        } else if bytes >= ONE_KB {
            let b: f64 = bytes as f64 / ONE_KB as f64;
            let r = (b * 8 as f64) / dur;
            return format!("{:<6.2} KBytes   {:<5.1} Kbits/sec", b, r).to_string();
        } else {
            let r = (bytes as f64 * 8 as f64) / dur;
            return format!("{:<6} Bytes    {:<5.1} bits/sec", bytes, r).to_string();
        }
    }
    pub fn server_header(&self) {
        println!(
            "Starting Test: protocol: {}, {} streams",
            self.conn(),
            self.num_streams()
        );
        println!("- - - - - - - - - - - - - - - - - - - - - - - - - - - - - - - - - -");
        match self.conn() {
            Conn::UDP => {
                println!("[ FD]  Time  Transfer        Rate             Lost/Total Datagrams")
            }
            Conn::TCP => println!("[ FD]  Time  Transfer        Rate             Rx packets"),
            Conn::QUIC => {
                println!("[ FD]  Time  Transfer        Rate              Total Datagrams")
            }
        }
        println!("- - - - - - - - - - - - - - - - - - - - - - - - - - - - - - - - - -");
    }
    pub fn client_header(&self) {
        println!(
            "Starting Test: protocol: {}, {} streams",
            self.conn(),
            self.num_streams()
        );
        println!("- - - - - - - - - - - - - - - - - - - - - - - - - - - - - - - - - -");
        match self.conn() {
            Conn::UDP => {
                println!("[ FD]  Time  Transfer        Rate             Total Datagrams")
            }
            Conn::TCP => println!("[ FD]  Time  Transfer        Rate             Tx packets"),
            Conn::QUIC => {
                println!("[ FD]  Time  Transfer        Rate              Total Datagrams")
            }
        }
        println!("- - - - - - - - - - - - - - - - - - - - - - - - - - - - - - - - - -");
    }
}

// impl Serialize for Test {
//     fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
//     where
//         S: Serializer,
//     {
//         // 3 is the number of fields in the struct.
//         let mut state = serializer.serialize_struct("Test", 1)?;
//         state.serialize_field("streams", serde_json::to_string(&self.streams).unwrap().as_bytes())?;
//         // state.serialize_field("g", &self.g)?;
//         // state.serialize_field("b", &self.b)?;
//         state.end()
//     }
// }
/*
state: TestState,
verbose: bool,
pub debug: bool,
skip_tls: bool,
pub cookie: String,
pub cookie_count: u8,
settings: Settings,
idle_timeout_in_secs: Option<Duration>,
reset_counter: u16,
pub streams: Vec<PerfStream>,
pub tokens: Vec<Token>,
pub start: Instant,
pub total_bytes: u64,
pub total_blks: u64,
*/
