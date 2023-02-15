use chrono::Local;
use mio::net::{TcpStream, UdpSocket};
use socket2::{Domain, Protocol, SockRef, Socket, Type};

use crate::quic::Quic;
use crate::{stream::Stream, test::TestState};
use std::io::{self, Read, Write};
use std::net::SocketAddr;
use std::os::unix::io::AsRawFd;

pub fn gettime() -> String {
    return Local::now().format("%Y-%m-%d %H:%M:%S.%6f").to_string();
}

pub fn make_addr(addr: &String, port: u16) -> String {
    String::from(addr) + ":" + &port.to_string()
}

pub fn write_socket(mut stream: &TcpStream, buf: &[u8]) -> io::Result<usize> {
    match stream.write(buf) {
        Ok(n) => {
            return Ok(n);
        }
        Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => {
            return Ok(0);
        }
        Err(e) => {
            return Err(e.into());
        }
    }
}

pub async fn read_socket(mut stream: &TcpStream) -> io::Result<String> {
    let mut buf = [0; 128 * 1024];
    match stream.read(&mut buf) {
        Ok(0) => {
            println!("Zero bytes read");
            return Ok(String::new());
        }
        Ok(n) => {
            let data = String::from_utf8(buf[0..n].to_vec()).unwrap().to_string();
            return Ok(data);
        }
        Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => {
            println!("Would block read socket {}", e);
            return Ok(e.to_string());
        }
        Err(e) => {
            println!("Some error {}", e);
            return Ok(e.to_string());
        }
    }
}

pub fn send_state(stream: &TcpStream, state: TestState) {
    let byte: &mut [u8] = &mut [state as u8];
    write_socket(&stream, byte).unwrap();
}

pub fn make_cookie() -> String {
    let rndchars: String = String::from("abcdefghijklmnopqrstuvwxyz234567");
    return rndchars;
}
/*
void make_cookie(const char *cookie) {
    unsigned char *out = (unsigned char *)cookie;
    size_t pos;
    static const unsigned char rndchars[] = "abcdefghijklmnopqrstuvwxyz234567";

    readentropy(out, COOKIE_SIZE);
    for (pos = 0; pos < (COOKIE_SIZE - 1); pos++) {
      out[pos] = rndchars[out[pos] % (sizeof(rndchars) - 1)];
    }
    out[pos] = '\0';
  }

pub fn set_nonblocking(stream: &TcpStream, nonblocking: bool) {
    let sck = SockRef::from(stream);
    match sck.set_nonblocking(nonblocking) {
        Ok(_) => return,
        Err(e) => {
            println!("Failed to set nonblocking {}", e.to_string());
            return;
        }
    }
}
*/

pub fn set_nodelay<T: Stream + AsRawFd + 'static>(stream: &T) {
    let sck = SockRef::from(stream);
    match sck.set_nodelay(true) {
        Ok(_) => return,
        Err(e) => {
            println!("Failed to set nodelay {}", e.to_string());
            return;
        }
    }
}

pub fn set_send_buffer_size<T: Stream + AsRawFd + 'static>(stream: &T, sz: usize) {
    let sck = SockRef::from(stream);
    match sck.set_send_buffer_size(sz) {
        Ok(_) => return,
        Err(e) => {
            println!("Failed to set send buffer size {}", e.to_string());
            return;
        }
    }
}
pub fn set_recv_buffer_size<T: Stream + AsRawFd + 'static>(stream: &T, sz: usize) {
    let sck = SockRef::from(stream);
    match sck.set_recv_buffer_size(sz) {
        Ok(_) => return,
        Err(e) => {
            println!("Failed to set recv buffer size {}", e.to_string());
            return;
        }
    }
}
pub fn mss(stream: &TcpStream) -> u32 {
    let sck = SockRef::from(stream);
    match sck.mss() {
        Ok(n) => return n,
        Err(e) => {
            println!("Failed to set nodelay {}", e.to_string());
            return 0;
        }
    }
}

pub fn print_tcp_stream(stream: &TcpStream) {
    let sck = SockRef::from(stream);
    println!(
        "[{:>3}] local {}, peer {} sndbuf {} rcvbuf {}",
        stream.as_raw_fd(),
        stream.local_addr().unwrap(),
        stream.peer_addr().unwrap(),
        sck.send_buffer_size().unwrap(),
        sck.recv_buffer_size().unwrap()
    );
}
pub fn print_udp_stream(stream: &UdpSocket) {
    let sck = SockRef::from(stream);
    println!(
        "[{:>3}] local {}, peer {} sndbuf {} rcvbuf {}",
        stream.as_raw_fd(),
        stream.local_addr().unwrap(),
        stream.peer_addr().unwrap(),
        sck.send_buffer_size().unwrap(),
        sck.recv_buffer_size().unwrap()
    );
}
pub fn print_quic_stream(stream: &Quic) {
    //let sck = SockRef::from(stream);
    println!(
        "[{:>3}] local {:?}, peer {}", // sndbuf {} rcvbuf {}",
        stream.fd,
        stream.conn.as_ref().unwrap().local_ip(),
        stream.conn.as_ref().unwrap().remote_address(),
        // sck.send_buffer_size().unwrap(),
        // sck.recv_buffer_size().unwrap()
    );
}

pub fn create_net_udp_socket(addr: SocketAddr) -> std::net::UdpSocket {
    let sck = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP)).unwrap();
    sck.set_reuse_address(true).unwrap();
    //sck.set_recv_buffer_size(63*1024).unwrap();
    sck.bind(&addr.into()).unwrap();
    std::net::UdpSocket::from(sck)
}
pub fn create_mio_udp_socket(addr: SocketAddr) -> UdpSocket {
    UdpSocket::from_std(create_net_udp_socket(addr))
}
