use mio::{Poll, Token};
use std::{
    any::Any,
    io::{self},
    os::unix::prelude::RawFd,
};

pub trait Stream {
    fn read(&mut self) -> io::Result<usize>;
    fn write(&mut self, buf: &[u8]) -> io::Result<usize>;
    fn fd(&self) -> RawFd;
    fn as_any(&self) -> &dyn Any;
    fn as_any_mut(&mut self) -> &mut dyn Any;
    fn register(&mut self, poll: &mut Poll, token: Token);
    fn deregister(&mut self, poll: &mut Poll);
    fn print(&self);
}
