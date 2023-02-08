use crate::params::PerfParams;
use crate::test::{PerfStream, Test, TestState};
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

        let zero_vec: Vec<u8> = vec![1; 128 * 1024];

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
                                let stream = TcpStream::connect(self.server_addr)?;
                                print_stream(&stream);
                                test.streams.push(PerfStream::new(stream));
                            }
                            test.transition(TestState::Wait);
                        }
                        TestState::TestRunning => {}
                        TestState::TestStart => {
                            for pstream in &mut test.streams {
                                match pstream.write(make_cookie().as_bytes()) {
                                    Ok(_) => {}
                                    Err(_e) => {
                                        continue;
                                    }
                                }
                            }
                            test.transition(TestState::TestRunning);
                            send_state(&self.ctrl, TestState::TestRunning);
                            for pstream in &mut test.streams {
                                poll.registry().register(
                                    &mut pstream.stream,
                                    STREAM,
                                    Interest::READABLE | Interest::WRITABLE,
                                )?;
                            }
                            println!(
                                "Starting Test: protocol: TCP, {} stream",
                                test.num_streams()
                            );
                            test.start = Instant::now();
                            for pstream in &mut test.streams {
                                pstream.curr_time = Instant::now();
                            }
                        }
                        TestState::TestEnd => {
                            for pstream in &test.streams {
                                pstream.stream.shutdown(std::net::Shutdown::Both)?;
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
                                while try_later == false {
                                    for pstream in &mut test.streams {
                                        match pstream.write(zero_vec.as_slice()) {
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
                                        }
                                    }
                                }
                                if test.start.elapsed().as_millis() > 10000 {
                                    test.transition(TestState::TestEnd);
                                    send_state(&self.ctrl, TestState::TestEnd);
                                    for pstream in &mut test.streams {
                                        poll.registry().deregister(&mut pstream.stream)?;
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
