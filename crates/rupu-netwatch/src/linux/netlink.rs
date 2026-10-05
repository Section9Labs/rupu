//! `NETLINK_SOCK_DIAG` socket (Linux only). A thin owner of a raw netlink
//! socket built on `rustix::net`; all framing lives in [`super::req`] and
//! [`super::parse`].

use std::io;

use rustix::fd::OwnedFd;
use rustix::net::netlink::{SocketAddrNetlink, SOCK_DIAG};
use rustix::net::{
    bind, recv, send, socket_with, sockopt, AddressFamily, RecvFlags, SendFlags, SocketFlags,
    SocketType,
};

use super::req::DESTROY_GROUPS;

/// A raw `NETLINK_SOCK_DIAG` socket.
#[derive(Debug)]
pub struct DiagSocket {
    fd: OwnedFd,
}

impl DiagSocket {
    /// Open a close-on-exec `AF_NETLINK`/`SOCK_RAW` socket for `sock_diag`.
    pub fn open() -> io::Result<Self> {
        let fd = socket_with(
            AddressFamily::NETLINK,
            SocketType::RAW,
            SocketFlags::CLOEXEC,
            Some(SOCK_DIAG),
        )?;
        Ok(Self { fd })
    }

    /// Bind to the socket-destroy multicast groups, letting the kernel pick
    /// the port id.
    pub fn bind_destroy_groups(&self) -> io::Result<()> {
        bind(&self.fd, &SocketAddrNetlink::new(0, DESTROY_GROUPS))?;
        Ok(())
    }

    /// Send one netlink request.
    pub fn send(&self, bytes: &[u8]) -> io::Result<()> {
        send(&self.fd, bytes, SendFlags::empty())?;
        Ok(())
    }

    /// Receive one datagram into `buf`, returning the bytes written.
    pub fn recv_into(&self, buf: &mut [u8]) -> io::Result<usize> {
        let (written, _) = recv(&self.fd, buf, RecvFlags::empty())?;
        Ok(written)
    }

    /// Best-effort `SO_RCVBUF` bump; errors are ignored.
    pub fn set_rcvbuf(&self, bytes: usize) {
        let _ = sockopt::set_socket_recv_buffer_size(&self.fd, bytes);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::linux::parse::{nlmsgs, parse_inet_diag};
    use crate::linux::req::dump_request;
    use crate::types::Transport;

    /// `NLMSG_DONE`.
    const NLMSG_DONE: u16 = 3;
    /// `NLMSG_ERROR`.
    const NLMSG_ERROR: u16 = 2;

    #[test]
    #[ignore = "needs a Linux kernel with sock_diag"]
    fn tcp_dump_yields_at_least_one_parsed_socket() {
        let sock = DiagSocket::open().expect("open sock_diag");
        sock.set_rcvbuf(1 << 20);
        sock.send(&dump_request(2, 6, 1)).expect("send dump");
        let mut buf = vec![0u8; 1 << 16];
        let mut parsed = 0usize;
        'outer: loop {
            let n = sock.recv_into(&mut buf).expect("recv");
            assert!(n > 0, "unexpected EOF");
            for (ty, body) in nlmsgs(&buf[..n]) {
                match ty {
                    NLMSG_DONE => break 'outer,
                    NLMSG_ERROR => panic!("kernel returned NLMSG_ERROR"),
                    _ => {
                        if parse_inet_diag(body, Transport::Tcp).is_some() {
                            parsed += 1;
                        }
                    }
                }
            }
        }
        assert!(parsed >= 1, "no TCP sockets parsed from the dump");
    }

    #[test]
    #[ignore = "needs a Linux kernel with sock_diag"]
    fn bind_destroy_groups_succeeds() {
        let sock = DiagSocket::open().expect("open sock_diag");
        sock.bind_destroy_groups().expect("bind destroy groups");
    }
}
