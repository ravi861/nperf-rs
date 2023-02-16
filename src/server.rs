use crate::params::PerfParams;
use crate::quic::{self, Quic};
use crate::test::{Conn, PerfStream, Test, TestState};
use mio::net::{TcpListener, TcpStream, UdpSocket};
use mio::{Events, Interest, Poll, Token, Waker};
use std::net::SocketAddr;
use std::time::{Duration, Instant};

use core::panic;

use crate::net::*;

const CONTROL: Token = Token(1024);
const TCP_LISTENER: Token = Token(1025);
const UDP_LISTENER: Token = Token(1026);
const QUIC_LISTENER: Token = Token(1027);
const TOKEN_START: usize = 0;
const ONE_SEC: Duration = Duration::from_millis(1000);

pub struct ServerImpl {
    addr: SocketAddr,
    listener: TcpListener,
    ctrl: Option<TcpStream>,
    u: Option<UdpSocket>,
    q: Option<Quic>,
}

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
            u: None,
            q: None,
        })
    }
    pub async fn run(&mut self, test: &mut Test) -> std::io::Result<i8> {
        let mut poll = Poll::new().unwrap();
        poll.registry().register(
            &mut self.listener,
            TCP_LISTENER,
            Interest::READABLE | Interest::WRITABLE,
        )?;
        let waker = Waker::new(poll.registry(), CONTROL)?;
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
            // println!("{:?}", events);
            for event in events.iter() {
                match event.token() {
                    TCP_LISTENER => match test.state() {
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
                            print_tcp_stream(&stream);
                            let token = Token(TOKEN_START + test.tokens.len());
                            test.tokens.push(token);
                            poll.registry().register(
                                &mut stream,
                                token,
                                Interest::READABLE | Interest::WRITABLE,
                            )?;
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
                                match test.conn() {
                                    Conn::UDP => {
                                        self.u = Some(create_mio_udp_socket(self.addr.into()));
                                        poll.registry().register(
                                            self.u.as_mut().unwrap(),
                                            UDP_LISTENER,
                                            Interest::READABLE | Interest::WRITABLE,
                                        )?;
                                    }
                                    Conn::QUIC => {
                                        self.q =
                                            Some(quic::server(self.addr.into(), test.skip_tls()));
                                        poll.registry().register(
                                            self.q.as_mut().unwrap(),
                                            QUIC_LISTENER,
                                            Interest::READABLE | Interest::WRITABLE,
                                        )?;
                                    }
                                    _ => {}
                                }
                                test.transition(TestState::CreateStreams);
                                send_state(ctrl_ref, TestState::CreateStreams);
                            }
                        }
                        TestState::TestStart => {
                            let ctrl_ref = self.ctrl.as_ref().unwrap();
                            let buf = read_socket(ctrl_ref).await?;
                            let state = TestState::from_i8(buf.as_bytes()[0] as i8);
                            test.transition(state);
                        }
                        TestState::TestRunning => {
                            let ctrl_ref = self.ctrl.as_ref().unwrap();
                            let buf = read_socket(ctrl_ref).await?;
                            let state = TestState::from_i8(buf.as_bytes()[0] as i8);
                            test.transition(state);
                            waker.wake()?;
                        }
                        TestState::CreateStreams => {}
                        TestState::TestEnd => {
                            for pstream in &mut test.streams {
                                if pstream.curr_bytes > 0 {
                                    pstream.push_stat();
                                }
                                // let q: &mut Quic = (&mut pstream.stream).into();
                                // println!("{:?}", q.conn.as_ref().unwrap().stats());
                            }
                            test.print_stats();
                            return Ok(0);
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
                    UDP_LISTENER => match test.state() {
                        TestState::CreateStreams => {
                            if event.is_readable() {
                                self.create_udp_stream(&mut poll, test).unwrap();

                                if test.streams.len() < test.num_streams() as usize {
                                    self.u = Some(create_mio_udp_socket(self.addr.into()));
                                    poll.registry()
                                        .register(
                                            self.u.as_mut().unwrap(),
                                            UDP_LISTENER,
                                            Interest::READABLE | Interest::WRITABLE,
                                        )
                                        .unwrap();
                                }
                                if test.streams.len() > test.num_streams() as usize {
                                    panic!("Incorrect parallel streams");
                                }
                                if test.streams.len() == test.num_streams() as usize {
                                    let ctrl_ref = self.ctrl.as_ref().unwrap();
                                    test.transition(TestState::TestStart);
                                    send_state(ctrl_ref, TestState::TestStart);
                                    println!(
                                        "Starting Test: protocol: UDP, {} streams",
                                        test.num_streams()
                                    );
                                }
                            }
                        }
                        _ => {}
                    },
                    QUIC_LISTENER => match test.state() {
                        TestState::CreateStreams => {
                            if event.is_readable() {
                                self.create_quic_stream(&mut poll, test).await.unwrap();

                                if test.streams.len() < test.num_streams() as usize {
                                    self.q = Some(quic::server(self.addr.into(), test.skip_tls()));
                                    poll.registry()
                                        .register(
                                            self.q.as_mut().unwrap(),
                                            QUIC_LISTENER,
                                            Interest::READABLE | Interest::WRITABLE,
                                        )
                                        .unwrap();
                                }
                                if test.streams.len() > test.num_streams() as usize {
                                    panic!("Incorrect parallel streams");
                                }
                                if test.streams.len() == test.num_streams() as usize {
                                    let ctrl_ref = self.ctrl.as_ref().unwrap();
                                    test.transition(TestState::TestStart);
                                    send_state(ctrl_ref, TestState::TestStart);
                                    println!(
                                        "Starting Test: protocol: QUIC, {} streams",
                                        test.num_streams()
                                    );
                                }
                                break;
                            }
                        }
                        _ => {}
                    },
                    token => match test.state() {
                        TestState::TestRunning => {
                            if event.is_readable() {
                                let conn = test.conn();
                                let pstream = &mut test.streams[token.0];
                                loop {
                                    let d = match conn {
                                        Conn::QUIC => {
                                            // println!("{}", pstream.blks);
                                            quic::read((&mut pstream.stream).into()).await
                                        }
                                        _ => pstream.read(),
                                    };
                                    match d {
                                        Ok(0) => break,
                                        Ok(n) => {
                                            // println!("{} bytes", n);
                                            pstream.bytes += n as u64;
                                            pstream.curr_bytes += n as u64;
                                            pstream.blks += 1;
                                            pstream.curr_blks += 1;
                                        }
                                        Err(_e) => break,
                                    }
                                    if pstream.curr_time.elapsed() > ONE_SEC {
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
                                match test.conn() {
                                    Conn::QUIC => {
                                        let mut count = 0;
                                        let pstream = &mut test.streams[token.0];
                                        let q: &mut Quic = (&mut pstream.stream).into();
                                        while count < 1 {
                                            let recv = q
                                                .conn
                                                .as_ref()
                                                .unwrap()
                                                .accept_uni()
                                                .await
                                                .unwrap();
                                            println!("Quic Accept UNI: {:?}", recv.id());
                                            q.recv_streams.push(recv);
                                            // Because it is a unidirectional stream, we can only receive not send back.
                                            let _n = quic::read_cookie(q).await.unwrap();
                                            println!("Cookie: {:?}", _n);
                                            count += 1;
                                        }
                                        pstream.curr_time = Instant::now();
                                        test.start = Instant::now();
                                    }
                                    _ => {
                                        let pstream = &mut test.streams[token.0];
                                        let _n = pstream.read()?;
                                        // println!("Cookie: {}", n);
                                        // keep this here for most recent start time of actual data
                                        pstream.curr_time = Instant::now();
                                        test.start = Instant::now();
                                    }
                                }
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
    }

    fn create_udp_stream(&mut self, poll: &mut Poll, test: &mut Test) -> Result<(), ()> {
        let mut buf = [0; 128 * 1024];
        let (_, sock_addr) = self.u.as_ref().unwrap().recv_from(&mut buf).unwrap();
        self.u.as_ref().unwrap().connect(sock_addr).unwrap();

        let token = Token(TOKEN_START + test.tokens.len());
        test.tokens.push(token);
        poll.registry()
            .reregister(self.u.as_mut().unwrap(), token, Interest::READABLE)
            .unwrap();

        print_udp_stream(self.u.as_ref().unwrap());
        test.streams.push(PerfStream::new(self.u.take().unwrap()));
        Ok(())
    }

    async fn create_quic_stream(&mut self, poll: &mut Poll, test: &mut Test) -> Result<(), ()> {
        let handshake = self.q.as_ref().unwrap().endpoint.accept().await.unwrap();
        let conn = handshake.await.unwrap();
        self.q.as_mut().unwrap().conn = Some(conn);

        let token = Token(TOKEN_START + test.tokens.len());
        test.tokens.push(token);
        poll.registry()
            .reregister(self.q.as_mut().unwrap(), token, Interest::READABLE)
            .unwrap();

        print_quic_stream(&self.q.as_ref().unwrap());
        test.streams.push(PerfStream::new(self.q.take().unwrap()));
        Ok(())
    }
}
