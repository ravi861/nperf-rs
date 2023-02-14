use crate::params::PerfParams;
use crate::quic::{self, Quic};
use crate::test::{Conn, PerfStream, Test, TestState};
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
                                match test.conn() {
                                    Conn::UDP => {
                                        let stream =
                                            UdpSocket::bind("127.0.0.1:0".parse().unwrap())
                                                .unwrap();
                                        stream.connect(self.server_addr).unwrap();
                                        print_udp_stream(&stream);
                                        stream.send("hello".as_bytes())?;
                                        test.streams.push(PerfStream::new(stream));
                                    }
                                    Conn::TCP => {
                                        let stream = TcpStream::connect(self.server_addr)?;
                                        print_tcp_stream(&stream);
                                        test.streams.push(PerfStream::new(stream));
                                    }
                                    Conn::QUIC => {
                                        let quic = quic::client(self.server_addr).await;
                                        print_quic_stream(&quic);
                                        test.streams.push(PerfStream::new(quic));
                                    }
                                }
                            }
                            test.transition(TestState::Wait);
                        }
                        TestState::TestRunning => {}
                        TestState::TestStart => {
                            match test.conn() {
                                Conn::QUIC => {
                                    thread::sleep(Duration::from_millis(10));
                                    for pstream in &mut test.streams {
                                        let q: &mut Quic = (&mut pstream.stream).into();
                                        let stream =
                                            q.conn.as_mut().unwrap().open_uni().await.unwrap();
                                        println!("Quic Open UNI: {:?}", stream.id());
                                        q.send_streams.push(stream);
                                        match quic::write(q, make_cookie().as_bytes()).await {
                                            Ok(_) => {
                                                println!("Sent Cookie");
                                            }
                                            Err(_e) => {
                                                println!("Failed to send cookie");
                                                continue;
                                            }
                                        }
                                        pstream.stream.register(&mut poll, STREAM);
                                        pstream.curr_time = Instant::now();
                                    }
                                }
                                _ => {
                                    for pstream in &mut test.streams {
                                        match pstream.write(make_cookie().as_bytes()) {
                                            Ok(_) => {}
                                            Err(_e) => {
                                                println!("Failed to send cookie");
                                                continue;
                                            }
                                        }
                                        pstream.stream.register(&mut poll, STREAM);
                                        pstream.curr_time = Instant::now();
                                    }
                                }
                            }
                            println!(
                                "Starting Test: protocol: {}, {} stream",
                                test.conn(),
                                test.num_streams()
                            );
                            test.transition(TestState::TestRunning);
                            send_state(&self.ctrl, TestState::TestRunning);
                            test.start = Instant::now();
                        }
                        TestState::TestEnd => {
                            match test.conn() {
                                Conn::TCP => {
                                    for pstream in &test.streams {
                                        let x: &TcpStream = (&pstream.stream).into();
                                        x.shutdown(std::net::Shutdown::Both)?;
                                    }
                                }
                                Conn::QUIC => {
                                    for pstream in &mut test.streams {
                                        let q: &mut Quic = (&mut pstream.stream).into();
                                        for stream in &mut q.send_streams {
                                            stream.finish().await.unwrap();
                                        }
                                    }
                                }
                                _ => {}
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
                                let buf: Vec<u8> = vec![1; 128 * 1024];
                                let mut try_later = false;
                                let conn = test.conn();
                                while try_later == false {
                                    for pstream in &mut test.streams {
                                        let d = match conn {
                                            Conn::QUIC => {
                                                let q: &mut Quic = (&mut pstream.stream).into();
                                                quic::write(q, &buf.as_slice()).await
                                            }
                                            _ => pstream.write(&buf.as_slice()),
                                        };
                                        match d {
                                            Ok(n) => {
                                                pstream.bytes += n as u64;
                                                pstream.blks += 1;
                                                pstream.curr_bytes += n as u64;
                                                pstream.curr_blks += 1;
                                            }
                                            Err(_e) => {
                                                //println!("Is there error");
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
                                            match conn {
                                                Conn::QUIC | Conn::UDP => try_later = true,
                                                _ => {}
                                            }
                                        }
                                    }
                                }
                                if test.start.elapsed().as_millis() > 10000 {
                                    test.transition(TestState::TestEnd);
                                    send_state(&self.ctrl, TestState::TestEnd);
                                    for pstream in &mut test.streams {
                                        pstream.stream.deregister(&mut poll);
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
