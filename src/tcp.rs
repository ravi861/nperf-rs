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
            "[{:>3}] local {}, peer {} sndbuf {} rcvbuf {} mss {}",
            self.as_raw_fd(),
            self.local_addr().unwrap(),
            self.peer_addr().unwrap(),
            sck.send_buffer_size().unwrap(),
            sck.recv_buffer_size().unwrap(),
            sck.mss().unwrap()
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

pub fn mss(stream: &TcpStream) -> u32 {
    let sck = SockRef::from(stream);
    match sck.mss() {
        Ok(n) => return n,
        Err(e) => {
            println!("Failed to get mss {}", e.to_string());
            return 0;
        }
    }
}

pub fn set_mss(stream: &TcpStream, mss: u32) -> io::Result<()> {
    let sck = SockRef::from(stream);
    match sck.set_mss(mss) {
        Ok(_) => Ok(()),
        Err(e) => {
            println!("Failed to set mss {}", mss);
            return Err(e);
        }
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
                            Ok(..) => {
                                // for some reason debug build act weird without this
                                poll.registry().deregister(&mut stream)?;
                                return Ok(stream);
                            }
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
                            Ok(..) => {
                                // for some reason debug build act weird without this
                                poll.registry().deregister(listener)?;
                                return Ok(stream);
                            }
                            Err(_) => continue,
                        }
                    }
                }
                _ => {}
            }
        }
    }
}

pub fn _bind(addr: std::net::SocketAddr, mss: u32) -> io::Result<TcpListener> {
    let sck = socket2::Socket::new(
        socket2::Domain::for_address(addr),
        socket2::Type::STREAM,
        Some(socket2::Protocol::TCP),
    )
    .unwrap();
    sck.set_reuse_address(true).unwrap();
    sck.set_mss(mss).unwrap();
    sck.bind(&addr.into()).unwrap();
    sck.listen(1024).unwrap();
    Ok(TcpListener::from_std(std::net::TcpListener::from(sck)))
}
