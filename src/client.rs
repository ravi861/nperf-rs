use crate::params::PerfParams;
use crate::test::{PerfStream, Test, TestState};
use mio::net::UdpSocket;
use mio::{net::TcpStream, Events, Interest, Poll, Token, Waker};

use std::net::SocketAddr;
use std::process::exit;
use std::time::{Duration, Instant};
use std::{io, thread};

use crate::net::*;

pub struct ClientImpl {
    server_addr: SocketAddr,
    ctrl: TcpStream,
}

const CONTROL: Token = Token(0);
const STREAM: Token = Token(1);

impl ClientImpl {
    pub fn new(params: &PerfParams) -> io::Result<ClientImpl> {
        println!("Connecting to {}", make_addr(&params.bindaddr, params.port));
        let addr = (make_addr(&params.bindaddr, params.port)).parse().unwrap();
        let ctrl = TcpStream::connect(addr)?;
        println!("Control Connection MSS: {}", mss(&ctrl));

        Ok(ClientImpl {
            server_addr: addr,
            ctrl,
        })
    }
    pub async fn run(&mut self, mut test: Test) -> io::Result<()> {
        let mut poll = Poll::new().unwrap();
        poll.registry().register(
            &mut self.ctrl,
            CONTROL,
            Interest::READABLE | Interest::WRITABLE,
        )?;
        let waker = Waker::new(poll.registry(), CONTROL)?;
        let mut events = Events::with_capacity(1024);

        loop {
            poll.poll(&mut events, Some(Duration::from_millis(100)))?;
            for event in events.iter() {
                match event.token() {
                    CONTROL => match test.state() {
                        TestState::Start => {
                            if event.is_readable() {}
                            if event.is_writable() {
                                write_socket(&self.ctrl, make_cookie().as_bytes())?;
                                test.transition(TestState::Wait);
                            }
                        }
                        TestState::ParamExchange => {
                            // Send params
                            write_socket(&self.ctrl, test.settings().as_bytes())?;
                            test.transition(TestState::Wait);
                        }
                        TestState::CreateStreams => {
                            // connect to the server listener
                            for _ in 0..test.num_streams() {
                                thread::sleep(Duration::from_millis(10));
                                if test.udp() {
                                    let stream =
                                        UdpSocket::bind("127.0.0.1:0".parse().unwrap()).unwrap();
                                    stream.connect(self.server_addr).unwrap();
                                    println!("{} {}", stream.local_addr()?, stream.peer_addr()?);
                                    stream.send("hello".as_bytes())?;
                                    test.streams.push(PerfStream::new_udp(stream));
                                } else {
                                    let stream = TcpStream::connect(self.server_addr)?;
                                    print_stream(&stream);
                                    test.streams.push(PerfStream::new(stream));
                                }
                            }
                            test.transition(TestState::Wait);
                        }
                        TestState::TestRunning => {}
                        TestState::TestStart => {
                            if test.udp() {
                                for pstream in &mut test.streams {
                                    match pstream.write_buf(make_cookie().as_bytes()) {
                                        Ok(_) => {}
                                        Err(_e) => {
                                            continue;
                                        }
                                    }
                                    poll.registry().register(
                                        pstream.udp_stream.as_mut().unwrap(),
                                        STREAM,
                                        Interest::WRITABLE,
                                    )?;
                                    pstream.curr_time = Instant::now();
                                }
                                println!(
                                    "Starting Test: protocol: UDP, {} stream",
                                    test.num_streams()
                                );
                            } else {
                                for pstream in &mut test.streams {
                                    match pstream.write_buf(make_cookie().as_bytes()) {
                                        Ok(_) => {}
                                        Err(_e) => {
                                            continue;
                                        }
                                    }
                                    poll.registry().register(
                                        pstream.stream.as_mut().unwrap(),
                                        STREAM,
                                        Interest::WRITABLE,
                                    )?;
                                    pstream.curr_time = Instant::now();
                                }
                                println!(
                                    "Starting Test: protocol: TCP, {} stream",
                                    test.num_streams()
                                );
                            }
                            test.transition(TestState::TestRunning);
                            send_state(&self.ctrl, TestState::TestRunning);
                            test.start = Instant::now();
                        }
                        TestState::TestEnd => {
                            if test.udp() {
                            } else {
                                for pstream in &test.streams {
                                    pstream
                                        .stream
                                        .as_ref()
                                        .unwrap()
                                        .shutdown(std::net::Shutdown::Both)?;
                                }
                            }
                            self.ctrl.shutdown(std::net::Shutdown::Both)?;
                            test.print_stats();
                            exit(0);
                        }
                        TestState::Wait => {
                            if event.is_readable() {
                                let buf = read_socket(&self.ctrl).await?;
                                let state = TestState::from_i8(buf.as_bytes()[0] as i8);
                                test.transition(state);
                                waker.wake()?;
                            }
                        }
                        _ => {
                            println!("Unexpected state {:?} for token {:?}", test.state(), event);
                            break;
                        }
                    },
                    STREAM => match test.state() {
                        TestState::TestRunning => {
                            if event.is_writable() {
                                let mut try_later = false;
                                let udp = test.udp();
                                while try_later == false {
                                    for pstream in &mut test.streams {
                                        // thread::sleep(Duration::from_millis(5));
                                        match pstream.write() {
                                            Ok(n) => {
                                                pstream.bytes += n as u64;
                                                pstream.blks += 1;
                                                pstream.curr_bytes += n as u64;
                                                pstream.curr_blks += 1;
                                            }
                                            Err(_e) => {
                                                try_later = true;
                                                break;
                                            }
                                        }
                                        if pstream.curr_time.elapsed().as_millis() > 999 {
                                            pstream.push_stat();
                                            pstream.curr_time = Instant::now();
                                            pstream.curr_bytes = 0;
                                            pstream.curr_blks = 0;
                                            pstream.curr_iter += 1;
                                            if udp {
                                                try_later = true;
                                            }
                                        }
                                    }
                                }
                                if test.start.elapsed().as_millis() > 10000 {
                                    test.transition(TestState::TestEnd);
                                    send_state(&self.ctrl, TestState::TestEnd);
                                    if test.udp() {
                                        for pstream in &mut test.streams {
                                            poll.registry()
                                                .deregister(pstream.udp_stream.as_mut().unwrap())?;
                                        }
                                    } else {
                                        for pstream in &mut test.streams {
                                            poll.registry()
                                                .deregister(pstream.stream.as_mut().unwrap())?;
                                        }
                                    }
                                    waker.wake()?;
                                }
                            }
                        }
                        _ => {}
                    },
                    _ => {}
                }
            }
        }
    }
}
