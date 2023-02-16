use crate::stream::Stream;
use mio::{net::TcpStream, Interest, Poll, Token};
use socket2::SockRef;
use std::{
    any::Any,
    io,
    os::unix::prelude::{AsRawFd, RawFd},
};

impl Stream for TcpStream {
    #[inline(always)]
    fn read(&mut self) -> io::Result<usize> {
        let mut buf = [0; 131072];
        std::io::Read::read(self, &mut buf)
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
    fn print(&self) {
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
