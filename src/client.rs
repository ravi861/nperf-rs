use crate::params::PerfParams;
use crate::quic::{self, Quic};
use crate::test::{Conn, PerfStream, Stream, Test, TestState, ONE_SEC};
use mio::net::{TcpStream, UdpSocket};
use mio::{Events, Interest, Poll, Token, Waker};
use std::net::SocketAddr;
use std::time::{Duration, Instant};
use std::{io, thread};

use crate::net::*;

const CONTROL: Token = Token(1024);
const STREAM: Token = Token(0);

pub struct ClientImpl {
    server_addr: SocketAddr,
    ctrl: TcpStream,
}

impl ClientImpl {
    pub fn new(params: &PerfParams) -> io::Result<ClientImpl> {
        println!("Connecting to {}", make_addr(&params.bindaddr, params.port));
        let addr = (make_addr(&params.bindaddr, params.port)).parse().unwrap();
        let ctrl = TcpStream::connect(addr)?;
        set_nodelay(&ctrl);
        set_linger(&ctrl);
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
            // println!("{:?}", events);
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
                                        stream.send("hello".as_bytes())?;
                                        stream.print_new_stream();
                                        test.streams.push(PerfStream::new(
                                            stream,
                                            crate::test::StreamMode::SENDER,
                                        ));
                                    }
                                    Conn::TCP => {
                                        let stream = TcpStream::connect(self.server_addr)?;
                                        stream.print_new_stream();
                                        test.streams.push(PerfStream::new(
                                            stream,
                                            crate::test::StreamMode::SENDER,
                                        ));
                                    }
                                    Conn::QUIC => {
                                        let stream =
                                            quic::client(self.server_addr, test.skip_tls()).await;
                                        stream.print_new_stream();
                                        test.streams.push(PerfStream::new(
                                            stream,
                                            crate::test::StreamMode::SENDER,
                                        ));
                                    }
                                }
                            }
                            test.transition(TestState::Wait);
                        }
                        TestState::TestStart => {
                            match test.conn() {
                                Conn::QUIC => {
                                    thread::sleep(Duration::from_millis(10));
                                    for pstream in &mut test.streams {
                                        for _ in 0..1 {
                                            thread::sleep(Duration::from_millis(500));
                                            let q: &mut Quic = (&mut pstream.stream).into();
                                            let stream =
                                                q.conn.as_mut().unwrap().open_uni().await.unwrap();
                                            println!("Quic Open UNI: {:?}", stream.id());
                                            q.send_streams.push(stream);
                                            match quic::write_cookie(q, make_cookie().as_bytes())
                                                .await
                                            {
                                                Ok(_) => {}
                                                Err(_e) => {
                                                    println!("Failed to send cookie");
                                                    continue;
                                                }
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
                            test.client_header();
                            test.start = Instant::now();
                            test.transition(TestState::TestRunning);
                            send_state(&self.ctrl, TestState::TestRunning);
                        }
                        TestState::TestRunning => {
                            // this state for this token can only be hit if the server is shutdown unplanned
                            for pstream in &mut test.streams {
                                if pstream.curr_bytes > 0 {
                                    pstream.push_stat(test.debug);
                                }
                            }
                            test.print_stats();
                            return Ok(());
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
                                            // println!("{:?}", stream);
                                            stream.finish().await.unwrap();
                                        }
                                        // println!("{:?}", q.conn.as_ref().unwrap().stats());
                                    }
                                }
                                _ => {}
                            }
                            self.ctrl.shutdown(std::net::Shutdown::Both)?;
                            for pstream in &mut test.streams {
                                if pstream.curr_bytes > 0 {
                                    pstream.push_stat(test.debug);
                                }
                            }
                            test.print_stats();
                            return Ok(());
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

                                // fetch test attributes
                                let conn = test.conn();
                                let test_bitrate = test.bitrate();
                                let test_bytes = test.bytes();
                                let test_blks = test.blks();
                                let len = test.length();
                                let test_time = test.time().clone();

                                // setup buffers
                                const TCP_BUF: [u8; 131072] = [1; 131072];
                                let mut UDP_BUF: [u8; 65500] = [1; 65500];
                                const QUIC_BUF: [u8; 65500] = [1; 65500];

                                while try_later == false {
                                    for pstream in &mut test.streams {
                                        if test_bitrate != 0 {
                                            let rate = (pstream.curr_bytes * 8) as f64
                                                / pstream.curr_time.elapsed().as_secs_f64();
                                            if rate as u64 > test_bitrate {
                                                continue;
                                                // } else {
                                                //     println!(
                                                //         "{:.6}",
                                                //         pstream.curr_time.elapsed().as_secs_f64()
                                                //     );
                                            }
                                        }
                                        let d = match conn {
                                            Conn::TCP => pstream.write(&TCP_BUF[..len]),
                                            Conn::UDP => {
                                                UDP_BUF[0..8]
                                                    .copy_from_slice(&pstream.blks.to_be_bytes());
                                                pstream.write(&UDP_BUF[..len])
                                            }
                                            Conn::QUIC => {
                                                let q: &mut Quic = (&mut pstream.stream).into();
                                                quic::write(q, &QUIC_BUF[..len]).await
                                            }
                                        };
                                        match d {
                                            Ok(n) => {
                                                pstream.bytes += n as u64;
                                                test.total_bytes += n as u64;
                                                pstream.blks += 1;
                                                test.total_blks += 1;
                                                pstream.curr_bytes += n as u64;
                                                pstream.curr_blks += 1;
                                            }
                                            Err(_e) => {
                                                //println!("Is there error");
                                                try_later = true;
                                                break;
                                            }
                                        }
                                        if (test_blks != 0) && (test.total_blks >= test_blks)
                                            || (test_bytes != 0) && (test.total_bytes >= test_bytes)
                                            || pstream.curr_time.elapsed() > ONE_SEC
                                        {
                                            pstream.push_stat(test.debug);
                                            pstream.curr_time = Instant::now();
                                            pstream.curr_bytes = 0;
                                            pstream.curr_blks = 0;
                                            pstream.curr_iter += 1;
                                            if test.start.elapsed() > test_time {
                                                try_later = true;
                                            }
                                            match conn {
                                                Conn::QUIC | Conn::UDP => try_later = true,
                                                _ => {}
                                            }
                                        }
                                    }
                                }
                                if (test_blks != 0) && (test.total_blks >= test_blks)
                                    || (test_bytes != 0) && (test.total_bytes >= test_bytes)
                                    || (test.start.elapsed() > test_time)
                                {
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
