use crate::test::{Conn, Stream};
use mio::{
    net::{TcpListener, TcpStream},
    Events, Interest, Poll, Token,
};
use socket2::SockRef;
use std::{
    any::Any,
    io,
    os::unix::prelude::{AsRawFd, RawFd},
    time::Duration,
};

impl Stream for TcpStream {
    #[inline(always)]
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        // let mut buf = [0; 131072];
        std::io::Read::read(self, buf)
    }
    #[inline(always)]
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        std::io::Write::write(self, buf)
    }
    fn fd(&self) -> RawFd {
        self.as_raw_fd()
    }
    fn register(&mut self, poll: &mut Poll, token: Token) {
        poll.registry()
            .register(self, token, Interest::WRITABLE | Interest::READABLE)
            .unwrap();
    }
    fn deregister(&mut self, poll: &mut Poll) {
        poll.registry().deregister(self).unwrap();
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
    fn print_new_stream(&self) {
        let sck = SockRef::from(self);
        println!(
            "[{:>3}] local {}, peer {} sndbuf {} rcvbuf {}",
            self.as_raw_fd(),
            self.local_addr().unwrap(),
            self.peer_addr().unwrap(),
            sck.send_buffer_size().unwrap(),
            sck.recv_buffer_size().unwrap()
        );
    }
    fn socket_type(&self) -> Conn {
        Conn::TCP
    }
}

impl<'a> From<&'a Box<dyn Stream>> for &'a TcpStream {
    fn from(s: &'a Box<dyn Stream>) -> &'a TcpStream {
        let b = match s.as_any().downcast_ref::<TcpStream>() {
            Some(b) => b,
            None => panic!("Stream is not a {}", stringify!(TcpStream)),
        };
        b
    }
}
impl<'a> From<&'a mut Box<dyn Stream>> for &'a mut TcpStream {
    fn from(s: &'a mut Box<dyn Stream>) -> &'a mut TcpStream {
        let b = match s.as_any_mut().downcast_mut::<TcpStream>() {
            Some(b) => b,
            None => panic!("Stream is not a {}", stringify!(TcpStream)),
        };
        b
    }
}

pub fn connect(addr: std::net::SocketAddr) -> io::Result<TcpStream> {
    loop {
        let mut poll = Poll::new().unwrap();
        let mut events = Events::with_capacity(1024);

        // mio TcpStream connect is non-blocking and returns unconnected stream
        let mut stream = TcpStream::connect(addr)?;
        poll.registry().register(
            &mut stream,
            Token(1),
            Interest::READABLE | Interest::WRITABLE,
        )?;

        poll.poll(&mut events, Some(Duration::from_secs(1)))?;
        for event in events.iter() {
            match event.token() {
                Token(1) => {
                    if event.is_writable() {
                        // If we can get a peer address it means the stream is
                        // connected.
                        match stream.peer_addr() {
                            Ok(..) => return Ok(stream),
                            Err(e) => {
                                println!("{}, retrying...", e.to_string());
                                std::thread::sleep(Duration::from_millis(1000));
                                continue;
                            }
                        }
                    }
                }
                _ => {}
            }
        }
    }
}

pub async fn accept(listener: &mut TcpListener) -> io::Result<TcpStream> {
    let mut poll = Poll::new().unwrap();
    let mut events = Events::with_capacity(1024);

    poll.registry()
        .register(listener, Token(1), Interest::READABLE | Interest::WRITABLE)?;

    loop {
        poll.poll(&mut events, Some(Duration::from_millis(10)))?;
        for event in events.iter() {
            match event.token() {
                Token(1) => {
                    if event.is_readable() {
                        let (stream, _) = listener.accept().unwrap();
                        // If we can get a peer address it means the stream is
                        // connected.
                        match stream.peer_addr() {
                            Ok(..) => return Ok(stream),
                            Err(_) => continue,
                        }
                    }
                }
                _ => {}
            }
        }
    }
}
