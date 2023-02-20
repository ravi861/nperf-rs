use crate::params::PerfParams;
use crate::quic::{self, Quic};
use crate::test::{Conn, PerfStream, Stream, StreamMode, Test, TestState, ONE_SEC};
use mio::net::{TcpListener, TcpStream, UdpSocket};
use mio::{Events, Interest, Poll, Token, Waker};
use std::net::SocketAddr;
use std::time::Instant;

use core::panic;

use crate::net::*;

const CONTROL: Token = Token(1024);
const TCP_LISTENER: Token = Token(1025);
const UDP_LISTENER: Token = Token(1026);
const QUIC_LISTENER: Token = Token(1027);
const TOKEN_START: usize = 0;

pub struct ServerImpl {
    addr: SocketAddr,
    ctrl: Option<TcpStream>,
    t: Option<TcpListener>,
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
            ctrl: None,
            t: Some(listener),
            u: None,
            q: None,
        })
    }
    pub async fn run(&mut self, test: &mut Test) -> std::io::Result<i8> {
        let mut poll = Poll::new().unwrap();
        let mut events = Events::with_capacity(128);

        let stream = crate::tcp::accept(self.t.as_mut().unwrap()).await?;
        if test.verbose() {
            println!("Time: {}", gettime());
        }
        println!("Accepted ctrl connection from {}", stream.peer_addr()?);
        set_nodelay(&stream);
        set_linger(&stream);
        self.ctrl = Some(stream);
        poll.registry()
            .register(self.ctrl.as_mut().unwrap(), CONTROL, Interest::READABLE)?;
        let waker = Waker::new(poll.registry(), CONTROL)?;

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
                    CONTROL => match test.state() {
                        TestState::Start => {
                            if event.is_readable() {
                                let ctrl_ref = self.ctrl.as_mut().unwrap();
                                let mut buf = [0; 32];
                                match ctrl_ref.read(&mut buf) {
                                    Ok(_) => {
                                        test.cookie =
                                            String::from_utf8(buf.to_vec()).unwrap().to_string()
                                    }
                                    Err(e) => {
                                        println!("Unable to read cookie, e {}", e.to_string());
                                        return Ok(2);
                                    }
                                };
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
                                    Conn::TCP => {
                                        // no need of a new tcp listener. Just use ctrl listener for tcp
                                        poll.registry().register(
                                            self.t.as_mut().unwrap(),
                                            TCP_LISTENER,
                                            Interest::READABLE | Interest::WRITABLE,
                                        )?;
                                    }
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
                                }
                                test.transition(TestState::CreateStreams);
                                send_state(ctrl_ref, TestState::CreateStreams);
                            }
                        }
                        // CreateStreams handled by each socket type below
                        TestState::CreateStreams => {}
                        // TestStart is managed by the data connection tokens
                        TestState::TestStart => {}
                        TestState::TestRunning => {
                            // this state for this token can only be hit if the client is shutdown unplanned
                            if event.is_readable() {
                                let ctrl_ref = self.ctrl.as_ref().unwrap();
                                let state = match read_socket(ctrl_ref).await {
                                    Ok(buf) => TestState::from_i8(buf.as_bytes()[0] as i8),
                                    Err(_) => TestState::TestEnd,
                                };
                                test.transition(state);
                                waker.wake()?;
                            }
                        }
                        TestState::TestEnd => {
                            for pstream in &mut test.streams {
                                if pstream.curr_bytes > 0 {
                                    pstream.push_stat(test.debug);
                                }
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
                    TCP_LISTENER => match test.state() {
                        TestState::CreateStreams => {
                            self.create_tcp_stream(&mut poll, test).unwrap();

                            if test.streams.len() > test.num_streams() as usize {
                                panic!("Incorrect parallel streams");
                            }
                            if test.streams.len() == test.num_streams() as usize {
                                let ctrl_ref = self.ctrl.as_ref().unwrap();
                                test.transition(TestState::TestStart);
                                send_state(ctrl_ref, TestState::TestStart);
                                test.server_header();
                            }
                        }
                        _ => {}
                    },
                    UDP_LISTENER => match test.state() {
                        TestState::CreateStreams => {
                            if event.is_readable() {
                                self.create_udp_stream(&mut poll, test).unwrap();

                                if test.streams.len() > test.num_streams() as usize {
                                    panic!("Incorrect parallel streams");
                                }
                                if test.streams.len() == test.num_streams() as usize {
                                    let ctrl_ref = self.ctrl.as_ref().unwrap();
                                    test.transition(TestState::TestStart);
                                    send_state(ctrl_ref, TestState::TestStart);
                                    test.server_header();
                                }
                            }
                        }
                        _ => {}
                    },
                    QUIC_LISTENER => match test.state() {
                        TestState::CreateStreams => {
                            if event.is_readable() {
                                self.create_quic_stream(&mut poll, test).await.unwrap();

                                if test.streams.len() > test.num_streams() as usize {
                                    panic!("Incorrect parallel streams");
                                }
                                if test.streams.len() == test.num_streams() as usize {
                                    let ctrl_ref = self.ctrl.as_ref().unwrap();
                                    test.transition(TestState::TestStart);
                                    send_state(ctrl_ref, TestState::TestStart);
                                    test.server_header();
                                }
                                break;
                            }
                        }
                        _ => {}
                    },
                    token => match test.state() {
                        TestState::TestRunning => {
                            if event.is_readable() {
                                // setup buffers
                                let mut buf: [u8; 131072] = [0; 131072];

                                let conn = test.conn();
                                let pstream = &mut test.streams[token.0];
                                loop {
                                    let d = match conn {
                                        Conn::QUIC => {
                                            quic::read((&mut pstream.stream).into()).await
                                        }
                                        Conn::TCP => pstream.read(&mut buf),
                                        Conn::UDP => pstream.read(&mut buf),
                                    };
                                    match d {
                                        Ok(0) => break,
                                        Ok(n) => {
                                            // println!("{} bytes", n);
                                            pstream.bytes += n as u64;
                                            pstream.curr_bytes += n as u64;
                                            pstream.blks += 1;
                                            pstream.curr_blks += 1;
                                            pstream.curr_last_blk =
                                                u64::from_be_bytes(buf[0..8].try_into().unwrap());
                                        }
                                        Err(_e) => break,
                                    }
                                    if pstream.curr_time.elapsed() > ONE_SEC {
                                        pstream.push_stat(test.debug);
                                        pstream.curr_time = Instant::now();
                                        pstream.curr_bytes = 0;
                                        pstream.curr_blks = 0;
                                        pstream.curr_iter += 1;
                                        pstream.prev_last_blk = pstream.curr_last_blk;
                                    }
                                }
                            }
                        }
                        TestState::TestStart => {
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
                                            let n = quic::read_cookie(q).await.unwrap();
                                            if test.debug {
                                                println!("Cookie: {:?}", n);
                                            }
                                            count += 1;
                                        }
                                        pstream.curr_time = Instant::now();
                                        test.start = Instant::now();
                                    }
                                    _ => {
                                        let pstream = &mut test.streams[token.0];
                                        let mut buf = [0; 32];
                                        let n = pstream.read(&mut buf)?;
                                        if test.debug {
                                            println!("Cookie: {:?}", n);
                                        }
                                        // keep this here for most recent start time of actual data
                                        pstream.curr_time = Instant::now();
                                        test.start = Instant::now();
                                    }
                                }
                                test.cookie_count += 1;
                                if test.num_streams() == test.cookie_count {
                                    test.transition(TestState::TestRunning);
                                    send_state(self.ctrl.as_ref().unwrap(), TestState::TestRunning);
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

    fn create_tcp_stream(&mut self, poll: &mut Poll, test: &mut Test) -> Result<(), ()> {
        let (mut stream, _) = self.t.as_ref().unwrap().accept().unwrap();

        let token = Token(TOKEN_START + test.tokens.len());
        test.tokens.push(token);
        poll.registry()
            .register(&mut stream, token, Interest::READABLE)
            .unwrap();

        stream.print_new_stream();
        test.streams
            .push(PerfStream::new(stream, StreamMode::RECEIVER));

        // no need of a new listener (unlike udp and quic)
        Ok(())
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

        self.u.as_ref().unwrap().print_new_stream();
        test.streams.push(PerfStream::new(
            self.u.take().unwrap(),
            StreamMode::RECEIVER,
        ));

        // recreate a new udp socket to wait for new streams
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

        self.q.as_ref().unwrap().print_new_stream();
        test.streams.push(PerfStream::new(
            self.q.take().unwrap(),
            StreamMode::RECEIVER,
        ));

        // recreate a new quic connection to wait for new streams
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
        Ok(())
    }
}
