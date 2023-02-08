use crate::test::{PerfStream, Test, TestState};
use crate::params::PerfParams;
use mio::net::{TcpListener, TcpStream, UdpSocket};
use mio::{Events, Interest, Poll, Token, Waker};
use std::net::SocketAddr;
use std::time::Instant;

use core::panic;

use crate::net::*;

pub struct ServerImpl {
    addr: SocketAddr,
    listener: TcpListener,
    ctrl: Option<TcpStream>,
    udp_listener: Option<UdpSocket>,
}
const LISTENER: Token = Token(1024);
const CONTROL: Token = Token(1025);
const UDP_LISTENER: Token = Token(1025);
const TOKEN_START: usize = 0;

impl ServerImpl {
    pub fn new(params: &PerfParams) -> std::io::Result<ServerImpl> {
        println!("==========================================");
        println!(
            "Server listening on {}",
            make_addr(&params.bindaddr, params.port)
        );
        println!("==========================================");
        let addr = (make_addr(&params.bindaddr, params.port)).parse().unwrap();
        // TODO: handle failure
        let listener = TcpListener::bind(addr)?;
        Ok(ServerImpl {
            addr,
            listener: listener,
            ctrl: None,
            udp_listener: None,
        })
    }
    pub async fn run(&mut self, test: &mut Test) -> std::io::Result<i8> {
        let mut poll = Poll::new().unwrap();
        poll.registry().register(
            &mut self.listener,
            LISTENER,
            Interest::READABLE | Interest::WRITABLE,
        )?;
        let waker = Waker::new(poll.registry(), LISTENER)?;
        let mut events = Events::with_capacity(128);

        loop {
            poll.poll(&mut events, test.idle_timeout())?;
            if events.is_empty() {
                println!(
                    "Server restart #{} after idle timeout {} sec",
                    test.reset_counter_inc(),
                    test.idle_timeout().unwrap().as_secs()
                );
                test.print_stats();
                return Ok(2);
            }
            for event in events.iter() {
                match event.token() {
                    LISTENER => match test.state() {
                        TestState::Start => {
                            let (stream, addr) = self.listener.accept().unwrap();
                            match self.ctrl {
                                None => {
                                    if test.verbose() {
                                        println!("Time: {}", gettime());
                                    }
                                    println!("Accepted ctrl connection from {}", addr);
                                    set_nodelay(&stream);
                                    self.ctrl = Some(stream);
                                    poll.registry().register(
                                        self.ctrl.as_mut().unwrap(),
                                        CONTROL,
                                        Interest::READABLE,
                                    )?;
                                }
                                _ => {
                                    println!("Denying ctrl connection from {}", addr);
                                    send_state(&stream, TestState::AccessDenied);
                                }
                            }
                        }
                        TestState::CreateStreams => {
                            // Collect all streams and get ready for running
                            let (mut stream, _) = self.listener.accept().unwrap();
                            print_stream(&stream);
                            let token = Token(TOKEN_START + test.tokens.len());
                            test.tokens.push(token);
                            poll.registry()
                                .register(&mut stream, token, Interest::READABLE)?;
                            test.streams.push(PerfStream::new(stream));
                            if test.streams.len() > test.num_streams() as usize {
                                panic!("Incorrect parallel streams");
                            }
                            if test.streams.len() == test.num_streams() as usize {
                                let ctrl_ref = self.ctrl.as_ref().unwrap();
                                test.transition(TestState::TestStart);
                                send_state(ctrl_ref, TestState::TestStart);
                                println!(
                                    "Starting Test: protocol: TCP, {} streams",
                                    test.num_streams()
                                );
                            }
                        }
                        TestState::TestStart | TestState::TestRunning => {}
                        TestState::TestEnd => {
                            test.print_stats();
                            return Ok(0);
                        }
                        _ => {
                            println!(
                                "Unexpected state {:?} for LISTENER ({:?})",
                                test.state(),
                                event
                            );
                            break;
                        }
                    },
                    CONTROL => match test.state() {
                        TestState::Start => {
                            if event.is_readable() {
                                let ctrl_ref = self.ctrl.as_ref().unwrap();
                                let buf = read_socket(ctrl_ref).await?;
                                test.cookie = buf;
                                test.transition(TestState::ParamExchange);
                                send_state(ctrl_ref, TestState::ParamExchange);
                            }
                        }
                        TestState::ParamExchange => {
                            if event.is_readable() {
                                let ctrl_ref = self.ctrl.as_ref().unwrap();
                                let buf = read_socket(ctrl_ref).await?;
                                test.set_settings(buf);
                                if test.verbose() {
                                    println!("\tCookie: {}", test.cookie);
                                    println!("\tTCP MSS: {}", test.mss());
                                }
                                if test.udp() {
                                    self.udp_listener = Some(UdpSocket::bind(self.addr)?);
                                }
                                test.transition(TestState::CreateStreams);
                                send_state(ctrl_ref, TestState::CreateStreams);
                            }
                        }
                        TestState::TestStart | TestState::TestRunning => {
                            let ctrl_ref = self.ctrl.as_ref().unwrap();
                            let buf = read_socket(ctrl_ref).await?;
                            let state = TestState::from_i8(buf.as_bytes()[0] as i8);
                            test.transition(state);
                            waker.wake()?;
                        }
                        _ => {
                            println!(
                                "Unexpected state {:?} for CONTROL ({:?})",
                                test.state(),
                                event.token()
                            );
                            break;
                        }
                    },
                    token => match test.state() {
                        TestState::TestRunning => {
                            if event.is_readable() {
                                loop {
                                    let pstream = &mut test.streams[token.0];
                                    // let mut buf = [0; 128 * 1024];
                                    match pstream.read() {
                                        Ok(0) => break,
                                        Ok(n) => {
                                            pstream.bytes += n as u64;
                                            pstream.curr_bytes += n as u64;
                                            pstream.blks += 1;
                                            pstream.curr_blks += 1;
                                        }
                                        Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                                            break
                                        }
                                        Err(_e) => break,
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
                        }
                        TestState::CreateStreams | TestState::TestStart => {
                            if event.is_readable() {
                                let pstream = &mut test.streams[token.0];
                                let _buf = read_socket(&pstream.stream).await?;
                                // println!("Cookie: {}", buf);
                                pstream.curr_time = Instant::now();
                                // keep this here for most recent start time of actual data
                                test.start = Instant::now();
                            }
                        }
                        TestState::TestEnd => {}
                        _ => {
                            println!(
                                "Unexpected state {:?} for STREAM ({:?})",
                                test.state(),
                                event.token()
                            );
                            break;
                        }
                    },
                }
            }
        }
        // println!("Server timeout, exiting!");
        //Ok(())
    }
}
