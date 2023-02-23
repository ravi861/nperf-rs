use crate::params::PerfParams;
use crate::quic::{self, Quic};
use crate::test::{Conn, PerfStream, Stream, StreamMode, Test, TestState, ONE_SEC};
use mio::net::{TcpStream, UdpSocket};
use mio::{Events, Interest, Poll, Token, Waker};
use std::net::{IpAddr, SocketAddr};
use std::str::FromStr;
use std::time::Duration;
use std::{io, thread};

use crate::net::*;

const CONTROL: Token = Token(1024);
const STREAM: Token = Token(0);

pub struct ClientImpl {
    server_addr: SocketAddr,
    ctrl: TcpStream,
    running: bool,
}

impl ClientImpl {
    pub fn new(params: &PerfParams) -> io::Result<ClientImpl> {
        let ip = match &params.client {
            None => IpAddr::from_str("0.0.0.0").unwrap(),
            Some(addr) => match IpAddr::from_str(&addr) {
                Ok(addr) => addr,
                Err(e) => {
                    println!("{}: {}", addr, e.to_string());
                    std::process::exit(1);
                }
            },
        };
        println!("Connecting to {}:{}", ip.to_string(), params.port);
        let addr = SocketAddr::new(ip, params.port);
        let ctrl = crate::tcp::connect(addr)?;
        set_nodelay(&ctrl);
        set_linger(&ctrl);
        set_nonblocking(&ctrl, false);
        println!("Control Connection MSS: {}", mss(&ctrl));

        Ok(ClientImpl {
            server_addr: addr,
            ctrl,
            running: false,
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

        // todo
        test.mode = StreamMode::SENDER;

        loop {
            poll.poll(&mut events, Some(Duration::from_millis(100)))?;
            // println!("{:?}", events);
            for event in events.iter() {
                match event.token() {
                    CONTROL => match test.state() {
                        TestState::Start => {
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
                            for _ in 0..test.num_streams() {
                                thread::sleep(Duration::from_millis(10));
                                match test.conn() {
                                    Conn::UDP => {
                                        let stream =
                                            UdpSocket::bind("[::]:0".parse().unwrap()).unwrap();
                                        stream.connect(self.server_addr).unwrap();
                                        stream.send("hello".as_bytes())?;
                                        stream.print_new_stream();
                                        test.streams.push(PerfStream::new(stream, test.mode()));
                                    }
                                    Conn::TCP => {
                                        let stream = crate::tcp::connect(self.server_addr)?;
                                        stream.print_new_stream();
                                        test.streams.push(PerfStream::new(stream, test.mode()));
                                    }
                                    Conn::QUIC => {
                                        let stream =
                                            quic::client(self.server_addr, test.skip_tls()).await;
                                        stream.print_new_stream();
                                        test.streams.push(PerfStream::new(stream, test.mode()));
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
                                    }
                                }
                            }
                            test.transition(TestState::Wait);
                        }
                        TestState::TestRunning => {
                            if self.running {
                                // this state for this token can only be hit if the server is shutdown unplanned
                                test.end(&mut poll);
                                test.print_stats();
                                return Ok(());
                            } else {
                                self.running = true;
                                test.client_header();
                                for pstream in &mut test.streams {
                                    pstream.stream.register(&mut poll, STREAM);
                                }
                                test.start();
                            }
                        }
                        TestState::ExchangeResults => {
                            if event.is_readable() {
                                let json = match read_socket(&self.ctrl).await {
                                    Ok(buf) => buf,
                                    Err(_) => continue,
                                };
                                if test.debug() {
                                    println!("{}", json);
                                }
                                test.from_serde(json);
                                test.transition(TestState::End);
                                send_state(&self.ctrl, TestState::End);
                                waker.wake()?;
                            }
                        }
                        TestState::End => {
                            self.ctrl.shutdown(std::net::Shutdown::Both)?;
                            test.print_stats();
                            return Ok(());
                        }
                        TestState::TestEnd | TestState::Wait => {
                            if event.is_readable() {
                                let buf = read_socket(&self.ctrl).await?;
                                let state = TestState::from_i8(buf.as_bytes()[0] as i8);
                                test.transition(state);
                                waker.wake()?;
                            }
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
                                let mut udp_buf: [u8; 65500] = [1; 65500];
                                const QUIC_BUF: [u8; 65500] = [1; 65500];

                                while try_later == false {
                                    for pstream in &mut test.streams {
                                        if test_bitrate != 0 {
                                            let rate = (pstream.temp.bytes * 8) as f64
                                                / pstream.timers.curr.elapsed().as_secs_f64();
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
                                                udp_buf[0..8].copy_from_slice(
                                                    &(pstream.data.blks + 1).to_be_bytes(),
                                                );
                                                pstream.write(&udp_buf[..len])
                                            }
                                            Conn::QUIC => {
                                                let q: &mut Quic = (&mut pstream.stream).into();
                                                quic::write(q, &QUIC_BUF[..len]).await
                                            }
                                        };
                                        match d {
                                            Ok(n) => {
                                                pstream.data.bytes += n as u64;
                                                test.data.bytes += n as u64;
                                                pstream.temp.bytes += n as u64;
                                                pstream.data.blks += 1;
                                                test.data.blks += 1;
                                                pstream.temp.blks += 1;
                                            }
                                            Err(_e) => {
                                                //println!("Is there error");
                                                try_later = true;
                                                break;
                                            }
                                        }
                                        if (test_blks != 0) && (test.data.blks >= test_blks)
                                            || (test_bytes != 0) && (test.data.bytes >= test_bytes)
                                            || pstream.timers.curr.elapsed() > ONE_SEC
                                        {
                                            pstream.push_stat(test.debug);
                                            if test.timers.start.elapsed() > test_time {
                                                try_later = true;
                                            }
                                            match conn {
                                                Conn::QUIC | Conn::UDP => try_later = true,
                                                _ => {}
                                            }
                                        }
                                    }
                                }
                                if (test_blks != 0) && (test.data.blks >= test_blks)
                                    || (test_bytes != 0) && (test.data.bytes >= test_bytes)
                                    || (test.timers.start.elapsed() > test_time)
                                {
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
                                            }
                                        }
                                        _ => {}
                                    }
                                    test.end(&mut poll);
                                    test.transition(TestState::ExchangeResults);
                                    send_state(&self.ctrl, TestState::TestEnd);
                                    // waker.wake()?;
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
