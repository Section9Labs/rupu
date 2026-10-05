//! `com.apple.network.statistics` kernel-control socket (macOS only).
//!
//! A thin owner of a `PF_SYSTEM` / `SYSPROTO_CONTROL` datagram socket
//! connected to the `ntstat` kernel control (the feed behind `nettop`;
//! usable by an unprivileged user). It only builds requests and moves bytes;
//! decoding lives in [`super::parse`]. Request layouts are xnu
//! `bsd/net/ntstat.h` (interface revision 9), as proven by the feasibility
//! spike.

use std::io;
use std::os::fd::{AsFd, AsRawFd, BorrowedFd, OwnedFd};
use std::time::Duration;

use nix::sys::socket::{
    connect, recv, send, setsockopt, socket, sockopt, AddressFamily, MsgFlags, SockFlag,
    SockProtocol, SockType, SysControlAddr,
};
use nix::sys::time::TimeVal;

use super::parse::{PROVIDER_TCP_KERNEL, PROVIDER_UDP_KERNEL};

/// Kernel-control name of the network-statistics provider.
const CONTROL_NAME: &str = "com.apple.network.statistics";

/// Request message types (`nstat_msg_type_t`).
pub const MSG_ADD_ALL_SRCS: u32 = 1002;
pub const MSG_QUERY_SRC: u32 = 1004;
pub const MSG_GET_SRC_DESC: u32 = 1005;

/// `nstat_msg_hdr`: `context` u64, `type` u32, `length` u16, `flags` u16.
pub const HDR_LEN: usize = 16;
/// `nstat_msg_add_all_srcs`: hdr, `filter` u64, `events` u64, `provider` u32,
/// `target_pid` i32, `target_uuid[16]`.
pub const ADD_ALL_SRCS_LEN: usize = 56;
/// `nstat_msg_query_src_req` / `nstat_msg_get_src_description`: hdr + `srcref` u64.
pub const SRCREF_REQ_LEN: usize = 24;

const ADD_ALL_FILTER_OFF: usize = 16;
const ADD_ALL_PROVIDER_OFF: usize = 32;
const SRCREF_OFF: usize = 16;

/// `NSTAT_FILTER_FLAGS_V1_USAGE`-equivalent: accept every interface class.
/// Deliberately without `USE_UPDATE_FOR_ADD` — sources must arrive as
/// `SRC_ADDED` (then `GET_SRC_DESC`), which the parser decodes.
const FILTER_ALL_INTERFACES: u64 = 0x1FFF;

/// Receive buffer requested for the socket (bursts of `SRC_ADDED` at
/// subscribe time are large). Best effort.
const RCVBUF_BYTES: usize = 8 << 20;

fn header(ty: u32, len: usize, context: u64) -> Vec<u8> {
    let mut m = vec![0u8; len];
    m[0..8].copy_from_slice(&context.to_ne_bytes());
    m[8..12].copy_from_slice(&ty.to_ne_bytes());
    m[12..14].copy_from_slice(&(len as u16).to_ne_bytes());
    // flags stay 0
    m
}

/// `ADD_ALL_SRCS` for one provider.
fn add_all_srcs_msg(provider: u32, context: u64) -> Vec<u8> {
    let mut m = header(MSG_ADD_ALL_SRCS, ADD_ALL_SRCS_LEN, context);
    m[ADD_ALL_FILTER_OFF..ADD_ALL_FILTER_OFF + 8]
        .copy_from_slice(&FILTER_ALL_INTERFACES.to_ne_bytes());
    // events @24 stay 0; target_pid/uuid stay 0 (all processes).
    m[ADD_ALL_PROVIDER_OFF..ADD_ALL_PROVIDER_OFF + 4].copy_from_slice(&provider.to_ne_bytes());
    m
}

/// A request that carries only a source reference (`GET_SRC_DESC`, `QUERY_SRC`).
fn srcref_msg(ty: u32, srcref: u64) -> Vec<u8> {
    let mut m = header(ty, SRCREF_REQ_LEN, 0);
    m[SRCREF_OFF..SRCREF_OFF + 8].copy_from_slice(&srcref.to_ne_bytes());
    m
}

/// The connected `ntstat` control socket.
#[derive(Debug)]
pub struct NstatSocket {
    fd: OwnedFd,
}

impl NstatSocket {
    /// Open and connect the control socket.
    pub fn open() -> io::Result<Self> {
        let fd = socket(
            AddressFamily::System,
            SockType::Datagram,
            SockFlag::empty(),
            // `KextControl` is nix's name for `SYSPROTO_CONTROL`.
            SockProtocol::KextControl,
        )?;
        // Resolves the control name to its id via the CTLIOCGINFO ioctl.
        let addr = SysControlAddr::from_name(fd.as_raw_fd(), CONTROL_NAME, 0)?;
        connect(fd.as_raw_fd(), &addr)?;
        // Best effort: a larger buffer avoids ENOBUFS on the initial burst.
        let _ = setsockopt(&fd, sockopt::RcvBuf, &RCVBUF_BYTES);
        Ok(Self { fd })
    }

    fn send_all(&self, msg: &[u8]) -> io::Result<()> {
        let n = send(self.fd.as_raw_fd(), msg, MsgFlags::empty())?;
        if n != msg.len() {
            return Err(io::Error::new(
                io::ErrorKind::WriteZero,
                "short write to ntstat control socket",
            ));
        }
        Ok(())
    }

    /// Subscribe to every TCP and UDP kernel source (`ADD_ALL_SRCS` per
    /// provider). Existing sockets then arrive as `SRC_ADDED`.
    pub fn subscribe_all(&self) -> io::Result<()> {
        for (i, provider) in [PROVIDER_TCP_KERNEL, PROVIDER_UDP_KERNEL]
            .into_iter()
            .enumerate()
        {
            self.send_all(&add_all_srcs_msg(provider, 100 + i as u64))?;
        }
        Ok(())
    }

    /// Ask for a source's descriptor (`GET_SRC_DESC`); the answer is a
    /// `SRC_DESC` message.
    pub fn request_description(&self, srcref: u64) -> io::Result<()> {
        self.send_all(&srcref_msg(MSG_GET_SRC_DESC, srcref))
    }

    /// Ask for a source's byte counters (`QUERY_SRC`); the answer is a
    /// `SRC_COUNTS` message. `u64::MAX` queries every source.
    pub fn query_counts(&self, srcref: u64) -> io::Result<()> {
        self.send_all(&srcref_msg(MSG_QUERY_SRC, srcref))
    }

    /// Receive one datagram into `buf`, returning its length. It may carry
    /// several messages (each 8-byte aligned). Blocks until data arrives or
    /// the [`set_recv_timeout`](Self::set_recv_timeout) expires, which
    /// surfaces as `ErrorKind::WouldBlock` (`EAGAIN`).
    pub fn recv_into(&self, buf: &mut [u8]) -> io::Result<usize> {
        Ok(recv(self.fd.as_raw_fd(), buf, MsgFlags::empty())?)
    }

    /// Set (or clear, with `None`) the receive timeout so a poller never
    /// blocks forever in [`recv_into`](Self::recv_into).
    pub fn set_recv_timeout(&self, d: Option<Duration>) -> io::Result<()> {
        let tv = match d {
            // A zero timeval means "block forever" to SO_RCVTIMEO; round a
            // zero request up to the smallest real timeout (1 us).
            Some(d) => {
                let us = d.as_micros().max(1);
                TimeVal::new((us / 1_000_000) as _, (us % 1_000_000) as _)
            }
            None => TimeVal::new(0, 0),
        };
        setsockopt(&self.fd, sockopt::ReceiveTimeout, &tv)?;
        Ok(())
    }
}

impl AsFd for NstatSocket {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.fd.as_fd()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::macos::parse::{parse_message, NstatMsg};

    fn u16_at(b: &[u8], at: usize) -> u16 {
        u16::from_ne_bytes(b[at..at + 2].try_into().unwrap())
    }
    fn u32_at(b: &[u8], at: usize) -> u32 {
        u32::from_ne_bytes(b[at..at + 4].try_into().unwrap())
    }
    fn u64_at(b: &[u8], at: usize) -> u64 {
        u64::from_ne_bytes(b[at..at + 8].try_into().unwrap())
    }

    /// Pins the byte layout the requests (and the parser's `HDR_LEN`) assume,
    /// so a wrong offset fails here rather than as a silent kernel EINVAL.
    #[test]
    fn request_layouts_match_xnu() {
        // Sizes are 8-aligned (the kernel pads each message to 8).
        assert_eq!(HDR_LEN, 16);
        assert_eq!(ADD_ALL_SRCS_LEN, 16 + 8 + 8 + 4 + 4 + 16);
        assert_eq!(ADD_ALL_SRCS_LEN % 8, 0);
        assert_eq!(SRCREF_REQ_LEN, 16 + 8);

        let add = add_all_srcs_msg(PROVIDER_UDP_KERNEL, 7);
        assert_eq!(add.len(), 56);
        assert_eq!(u64_at(&add, 0), 7, "context@0");
        assert_eq!(u32_at(&add, 8), MSG_ADD_ALL_SRCS, "type@8");
        assert_eq!(u16_at(&add, 12), 56, "length@12");
        assert_eq!(u16_at(&add, 14), 0, "flags@14");
        assert_eq!(u64_at(&add, 16), FILTER_ALL_INTERFACES, "filter@16");
        assert_eq!(u64_at(&add, 24), 0, "events@24");
        assert_eq!(u32_at(&add, 32), PROVIDER_UDP_KERNEL, "provider@32");
        assert!(add[36..].iter().all(|&b| b == 0), "target pid/uuid zero");

        for (ty, srcref) in [(MSG_GET_SRC_DESC, 0xABCDu64), (MSG_QUERY_SRC, u64::MAX)] {
            let m = srcref_msg(ty, srcref);
            assert_eq!(m.len(), 24);
            assert_eq!(u32_at(&m, 8), ty);
            assert_eq!(u16_at(&m, 12), 24);
            assert_eq!(u64_at(&m, 16), srcref, "srcref@16");
        }
        assert_eq!(MSG_ADD_ALL_SRCS, 1002);
        assert_eq!(MSG_QUERY_SRC, 1004);
        assert_eq!(MSG_GET_SRC_DESC, 1005);
    }

    /// Live feed on this Mac: subscribe, see `SRC_ADDED`, ask for its
    /// description, see a decoded `SRC_DESC`.
    /// Run with `cargo test -p rupu-netwatch -- --ignored macos::ntstat`.
    #[test]
    #[ignore = "needs the live macOS network-statistics control"]
    fn live_feed_yields_added_then_desc() {
        let sock = NstatSocket::open().expect("open ntstat control socket");
        sock.set_recv_timeout(Some(Duration::from_millis(200)))
            .unwrap();
        sock.subscribe_all().expect("subscribe_all");

        let mut buf = vec![0u8; 1 << 17];
        let (mut added, mut descs, mut requested) = (0usize, 0usize, 0usize);
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while std::time::Instant::now() < deadline && descs == 0 {
            let n = match sock.recv_into(&mut buf) {
                Ok(n) => n,
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => continue,
                Err(e) => panic!("recv: {e}"),
            };
            let mut off = 0usize;
            while off + HDR_LEN <= n {
                let len = u16_at(&buf, off + 12) as usize;
                if len < HDR_LEN || off + len > n {
                    break;
                }
                match parse_message(&buf[off..off + len]) {
                    NstatMsg::SrcAdded { srcref, .. } => {
                        added += 1;
                        if requested < 50 {
                            requested += 1;
                            sock.request_description(srcref).unwrap();
                        }
                    }
                    NstatMsg::SrcDesc(obs) => {
                        descs += 1;
                        eprintln!("desc: {obs:?}");
                    }
                    _ => {}
                }
                off += (len + 7) & !7;
            }
        }
        eprintln!("added={added} descs={descs}");
        assert!(added > 0, "no SrcAdded observed");
        assert!(descs > 0, "no SrcDesc observed after request_description");
    }
}
