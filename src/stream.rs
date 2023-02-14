use mio::{
    net::{TcpStream, UdpSocket},
    Interest, Poll, Token,
};
use std::{
    any::Any,
    io::{self},
    os::unix::prelude::{AsRawFd, RawFd},
};

pub trait Stream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize>;
    fn write(&mut self, buf: &[u8]) -> io::Result<usize>;
    fn fd(&self) -> RawFd;
    fn as_any(&self) -> &dyn Any;
    fn as_any_mut(&mut self) -> &mut dyn Any;
    fn register(&mut self, poll: &mut Poll, token: Token);
    fn deregister(&mut self, poll: &mut Poll);
}

impl Stream for TcpStream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        std::io::Read::read(self, buf)
    }
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
}

impl Stream for UdpSocket {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.recv(buf)
    }
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if buf.len() >= 65536 {
            self.send(&buf[..63 * 1024])
        } else {
            self.send(buf)
        }
    }
    fn fd(&self) -> RawFd {
        self.as_raw_fd()
    }
    fn register(&mut self, poll: &mut Poll, token: Token) {
        poll.registry()
            .register(self, token, Interest::WRITABLE)
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
impl<'a> From<&'a Box<dyn Stream>> for &'a UdpSocket {
    fn from(s: &'a Box<dyn Stream>) -> &'a UdpSocket {
        let b = match s.as_any().downcast_ref::<UdpSocket>() {
            Some(b) => b,
            None => panic!("Stream is not a {}", stringify!(UdpSocket)),
        };
        b
    }
}
impl<'a> From<&'a mut Box<dyn Stream>> for &'a mut UdpSocket {
    fn from(s: &'a mut Box<dyn Stream>) -> &'a mut UdpSocket {
        let b = match s.as_any_mut().downcast_mut::<UdpSocket>() {
            Some(b) => b,
            None => panic!("Stream is not a {}", stringify!(UdpSocket)),
        };
        b
    }
}
