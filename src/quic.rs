use std::any::Any;
use std::fs::{self};
use std::io::{self, Error};
use std::net::{SocketAddr, UdpSocket};
use std::os::unix::prelude::{AsRawFd, RawFd};
use std::sync::Arc;
use std::time::Duration;

use crate::net::*;
use crate::noprotection::{NoProtectionClientConfig, NoProtectionServerConfig};
use crate::test::{Conn, Stream};
use mio::unix::SourceFd;
use mio::{event, Interest, Poll, Registry, Token};
use quinn::{AsyncStdRuntime, Connection, Endpoint, RecvStream, SendStream};

use bytes::Bytes;

pub static PERF_CIPHER_SUITES: &[rustls::SupportedCipherSuite] = &[
    rustls::cipher_suite::TLS13_AES_128_GCM_SHA256,
    rustls::cipher_suite::TLS13_AES_256_GCM_SHA384,
    rustls::cipher_suite::TLS13_CHACHA20_POLY1305_SHA256,
];

pub struct Quic {
    pub endpoint: Endpoint,
    pub fd: RawFd,
    pub conn: Option<Connection>,
    pub send_streams: Vec<SendStream>,
    pub recv_streams: Vec<RecvStream>,
}

impl event::Source for Quic {
    fn register(
        &mut self,
        registry: &Registry,
        token: Token,
        interests: Interest,
    ) -> io::Result<()> {
        SourceFd(&self.fd).register(registry, token, interests)
    }

    fn reregister(
        &mut self,
        registry: &Registry,
        token: Token,
        interests: Interest,
    ) -> io::Result<()> {
        SourceFd(&self.fd).reregister(registry, token, interests)
    }

    fn deregister(&mut self, registry: &Registry) -> io::Result<()> {
        SourceFd(&self.fd).deregister(registry)
    }
}

impl Stream for Quic {
    fn read(&mut self, _buf: &mut [u8]) -> io::Result<usize> {
        //let recv = self.conn.as_ref().unwrap().accept_uni();
        //std::io::Read::read(self, buf)
        Ok(0)
    }
    fn write(&mut self, _buf: &[u8]) -> io::Result<usize> {
        //let send = self.conn.as_ref().unwrap().open_uni();
        //std::io::Write::write(self, buf)
        Ok(0)
    }
    fn fd(&self) -> RawFd {
        self.fd
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
    fn print_new_stream(&self) {
        //let sck = SockRef::from(stream);
        println!(
            "[{:>3}] local {:?}, peer {}", // sndbuf {} rcvbuf {}",
            self.fd,
            self.conn.as_ref().unwrap().local_ip(),
            self.conn.as_ref().unwrap().remote_address(),
            // sck.send_buffer_size().unwrap(),
            // sck.recv_buffer_size().unwrap()
        );
    }
    fn socket_type(&self) -> Conn {
        Conn::QUIC
    }
}

impl<'a> From<&'a Box<dyn Stream>> for &'a Quic {
    fn from(s: &'a Box<dyn Stream>) -> &'a Quic {
        let b = match s.as_any().downcast_ref::<Quic>() {
            Some(b) => b,
            None => panic!("Stream is not a {}", stringify!(Quic)),
        };
        b
    }
}
impl<'a> From<&'a mut Box<dyn Stream>> for &'a mut Quic {
    fn from(s: &'a mut Box<dyn Stream>) -> &'a mut Quic {
        let b = match s.as_any_mut().downcast_mut::<Quic>() {
            Some(b) => b,
            None => panic!("Stream is not a {}", stringify!(Quic)),
        };
        b
    }
}

pub fn server(addr: SocketAddr, skip_tls: bool) -> Quic {
    let (key, cert) = match ("cert.key", "cert.crt") {
        (&ref key, &ref cert) => {
            let key = fs::read(key).unwrap();
            let cert = fs::read(cert).unwrap();

            let mut certs = Vec::new();
            for cert in rustls_pemfile::certs(&mut cert.as_ref()).unwrap() {
                certs.push(rustls::Certificate(cert));
            }

            let mut keys = Vec::new();
            for key in rustls_pemfile::pkcs8_private_keys(&mut key.as_ref()).unwrap() {
                keys.push(key);
            }
            (rustls::PrivateKey(keys.remove(0)), certs)
        }
        _ => {
            let cert = rcgen::generate_simple_self_signed(vec!["localhost".into()]).unwrap();
            (
                rustls::PrivateKey(cert.serialize_private_key_der()),
                vec![rustls::Certificate(cert.serialize_der().unwrap())],
            )
        }
    };

    let mut crypto = rustls::ServerConfig::builder()
        .with_cipher_suites(PERF_CIPHER_SUITES)
        .with_safe_default_kx_groups()
        .with_protocol_versions(&[&rustls::version::TLS13])
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(cert, key)
        .unwrap();
    crypto.alpn_protocols = vec![b"perf".to_vec()];

    let mut transport = quinn::TransportConfig::default();
    transport.initial_max_udp_payload_size(1200);
    transport.max_idle_timeout(Some(Duration::from_secs(1).try_into().unwrap()));

    let mut server_config = if skip_tls {
        quinn::ServerConfig::with_crypto(Arc::new(NoProtectionServerConfig::new(Arc::new(crypto))))
    } else {
        quinn::ServerConfig::with_crypto(Arc::new(crypto))
    };
    server_config.transport_config(Arc::new(transport));

    let socket = create_net_udp_socket(addr);
    let fd = socket.as_raw_fd();
    let endpoint = quinn::Endpoint::new(
        Default::default(),
        Some(server_config),
        socket,
        quinn::AsyncStdRuntime,
    )
    .unwrap();

    Quic {
        endpoint,
        fd,
        conn: None,
        send_streams: Vec::new(),
        recv_streams: Vec::new(),
    }
}

pub async fn client(addr: SocketAddr, skip_tls: bool) -> Quic {
    let socket = UdpSocket::bind("[::]:0".parse::<SocketAddr>().unwrap()).unwrap();
    let fd = socket.as_raw_fd();
    let endpoint = quinn::Endpoint::new(Default::default(), None, socket, AsyncStdRuntime).unwrap();

    let mut crypto = rustls::ClientConfig::builder()
        .with_cipher_suites(PERF_CIPHER_SUITES)
        .with_safe_default_kx_groups()
        .with_protocol_versions(&[&rustls::version::TLS13])
        .unwrap()
        .with_custom_certificate_verifier(SkipServerVerification::new())
        .with_no_client_auth();
    crypto.alpn_protocols = vec![b"perf".to_vec()];

    let mut transport = quinn::TransportConfig::default();
    transport.initial_max_udp_payload_size(1200);
    transport.max_idle_timeout(Some(Duration::from_secs(1).try_into().unwrap()));

    let mut cfg = if skip_tls {
        quinn::ClientConfig::new(Arc::new(NoProtectionClientConfig::new(Arc::new(crypto))))
    } else {
        quinn::ClientConfig::new(Arc::new(crypto))
    };

    cfg.transport_config(Arc::new(transport));
    let conn = endpoint
        .connect_with(cfg, addr, "perf")
        .unwrap()
        .await
        .unwrap();

    Quic {
        endpoint,
        fd,
        conn: Some(conn),
        send_streams: Vec::new(),
        recv_streams: Vec::new(),
    }
}

pub async fn read_cookie(q: &mut Quic) -> io::Result<usize> {
    let mut buf = [0; 128];
    let mut count = 0;
    for stream in &mut q.recv_streams {
        match stream.read(&mut buf).await {
            Ok(n) => match n {
                Some(n) => {
                    if n == 0 {
                        println!("Zero read");
                    }
                    count += n;
                }
                None => {
                    println!("Zero read quic");
                    return Err(Error::last_os_error());
                }
            },
            Err(_e) => {
                println!("{:?}", _e);
                return Err(Error::last_os_error());
            }
        }
    }
    Ok(count)
}

pub async fn read(q: &mut Quic) -> io::Result<usize> {
    // let mut bufs = [
    //     Bytes::new(), Bytes::new(), Bytes::new(), Bytes::new(),
    //     Bytes::new(), Bytes::new(), Bytes::new(), Bytes::new(),
    //     Bytes::new(), Bytes::new(), Bytes::new(), Bytes::new(),
    //     Bytes::new(), Bytes::new(), Bytes::new(), Bytes::new(),
    //     Bytes::new(), Bytes::new(), Bytes::new(), Bytes::new(),
    //     Bytes::new(), Bytes::new(), Bytes::new(), Bytes::new(),
    //     Bytes::new(), Bytes::new(), Bytes::new(), Bytes::new(),
    //     Bytes::new(), Bytes::new(), Bytes::new(), Bytes::new(),
    // ];
    let mut buf = [0; 131072];
    let mut count = 0;
    for stream in &mut q.recv_streams {
        // while let Some(size) = stream.read_chunks(&mut bufs[..]).await? {
        //     let bytes_received: usize = bufs[..size].iter().map(|b| b.len()).sum();
        //     count += bytes_received;
        // }
        // match stream.read_chunks(&mut bufs[..]).await {
        match stream.read(&mut buf).await {
            Ok(n) => match n {
                Some(n) => {
                    count += n;
                }
                None => {
                    // println!("Zero read quic");
                    return Err(Error::last_os_error());
                }
            },
            Err(_e) => {
                println!("{:?}", _e);
                return Err(Error::last_os_error());
            }
        }
    }
    Ok(count)
}

pub async fn write_cookie(q: &mut Quic, buf: &[u8]) -> io::Result<usize> {
    for stream in &mut q.send_streams {
        match stream.write(buf).await {
            Ok(_) => {}
            Err(_e) => return Err(Error::last_os_error()),
        }
    }
    Ok(buf.len() * q.send_streams.len())
}

pub async fn write(q: &mut Quic, buf: &'static [u8]) -> io::Result<usize> {
    for stream in &mut q.send_streams {
        match stream.write_chunk(Bytes::from_static(&buf)).await {
            // match stream.write(buf).await {
            Ok(_) => {}
            Err(_e) => return Err(Error::last_os_error()),
        }
    }
    Ok(buf.len() * q.send_streams.len())
}

struct SkipServerVerification;

impl SkipServerVerification {
    fn new() -> Arc<Self> {
        Arc::new(Self)
    }
}

impl rustls::client::ServerCertVerifier for SkipServerVerification {
    fn verify_server_cert(
        &self,
        _end_entity: &rustls::Certificate,
        _intermediates: &[rustls::Certificate],
        _server_name: &rustls::ServerName,
        _scts: &mut dyn Iterator<Item = &[u8]>,
        _ocsp_response: &[u8],
        _now: std::time::SystemTime,
    ) -> Result<rustls::client::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::ServerCertVerified::assertion())
    }
}
