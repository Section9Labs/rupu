//! Pure decoder for `com.apple.network.statistics` (`ntstat`) messages.
//!
//! Everything here reads `&[u8]` only: no sockets, no libproc, no `cfg`. It
//! is tested against real kernel output in `tests/fixtures/`. Layouts are
//! xnu `bsd/net/ntstat.h` (interface revision 9); all integers are native
//! (little-endian on every supported Mac).

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

use crate::types::Transport;

/// Message types (`nstat_msg_hdr.type`).
pub const MSG_SRC_ADDED: u32 = 10001;
pub const MSG_SRC_REMOVED: u32 = 10002;
pub const MSG_SRC_DESC: u32 = 10003;
pub const MSG_SRC_COUNTS: u32 = 10004;
pub const MSG_SRC_UPDATE: u32 = 10006;

/// Providers (`nstat_provider_id_t`).
pub const PROVIDER_TCP_KERNEL: u32 = 2;
pub const PROVIDER_UDP_KERNEL: u32 = 4;

/// One socket as described by an `ntstat` descriptor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NstatObservation {
    /// The kernel's source reference (the tracker's `SocketId`).
    pub srcref: u64,
    /// TCP or UDP.
    pub transport: Transport,
    /// Owning process id.
    pub pid: u32,
    /// Process name from the descriptor (at most 64 bytes, NUL-trimmed).
    pub pname: String,
    /// Local end, when the descriptor carries one.
    pub local: Option<SocketAddr>,
    /// Remote end; `None` for a listener or unconnected UDP socket.
    pub remote: Option<SocketAddr>,
    /// BSD TCP state (`TCPS_*`); 0 for UDP.
    pub state: u32,
    /// TCP reached ESTABLISHED or later; for UDP, the socket is connected.
    pub established: bool,
    /// Bytes received. Descriptors carry no counters, so this is `None`
    /// until a `SRC_COUNTS` is applied.
    pub bytes_in: Option<u64>,
    /// Bytes sent (see `bytes_in`).
    pub bytes_out: Option<u64>,
}

/// The message kinds the watcher acts on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NstatMsg {
    /// A new source appeared.
    SrcAdded { srcref: u64, provider: u32 },
    /// A source's descriptor (answer to `GET_SRC_DESC`, or a `SRC_UPDATE`).
    SrcDesc(NstatObservation),
    /// A source's byte counters (answer to `QUERY_SRC`).
    SrcCounts {
        srcref: u64,
        bytes_in: u64,
        bytes_out: u64,
    },
    /// A source went away.
    SrcRemoved { srcref: u64 },
    /// Anything else, or anything malformed.
    Other,
}

/// Size of `nstat_msg_hdr` (`context` u64, `type` u32, `length` u16, `flags` u16).
const HDR_LEN: usize = 16;
/// `nstat_msg_src_description` up to the descriptor: hdr + srcref + event
/// flags + provider (+ 4 reserved), 8-aligned.
const SRC_DESC_HDR_LEN: usize = 40;

/// BSD address families as they appear in `sockaddr.sa_family`.
const AF_INET: u8 = 2;
const AF_INET6: u8 = 30;

/// `sockaddr_in` is 16 bytes, `sockaddr_in6` 28; the descriptor reserves 28.
const SOCKADDR_UNION_LEN: usize = 28;

/// Offsets inside `nstat_tcp_descriptor`.
const TCP_STATE: usize = 76;
const TCP_PID: usize = 116;
const TCP_LOCAL: usize = 124;
const TCP_REMOTE: usize = 152;
const TCP_PNAME: usize = 196;

/// Offsets inside `nstat_udp_descriptor`.
const UDP_LOCAL: usize = 56;
const UDP_REMOTE: usize = 84;
const UDP_PID: usize = 128;
const UDP_PNAME: usize = 132;

const PNAME_LEN: usize = 64;

/// `TCPS_ESTABLISHED` .. `TCPS_TIME_WAIT`: every state past the handshake.
const TCPS_ESTABLISHED: u32 = 4;
const TCPS_TIME_WAIT: u32 = 10;

fn u16_le(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(at..at + 2)?.try_into().ok()?))
}

fn u32_le(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

fn u64_le(b: &[u8], at: usize) -> Option<u64> {
    Some(u64::from_le_bytes(b.get(at..at + 8)?.try_into().ok()?))
}

/// Decode the `sockaddr_in` / `sockaddr_in6` union at `at`. `None` when the
/// family is neither (an unset address) or the buffer is too short.
fn sockaddr(b: &[u8], at: usize) -> Option<SocketAddr> {
    let sa = b.get(at..at + SOCKADDR_UNION_LEN)?;
    let port = u16::from_be_bytes([sa[2], sa[3]]);
    match sa[1] {
        AF_INET => {
            let ip = Ipv4Addr::new(sa[4], sa[5], sa[6], sa[7]);
            Some(SocketAddr::new(IpAddr::V4(ip), port))
        }
        AF_INET6 => {
            let mut o = [0u8; 16];
            o.copy_from_slice(&sa[8..24]);
            Some(SocketAddr::new(IpAddr::V6(Ipv6Addr::from(o)), port))
        }
        _ => None,
    }
}

/// A remote of `0.0.0.0:0` / `[::]:0` means "not connected".
fn remote_addr(b: &[u8], at: usize) -> Option<SocketAddr> {
    sockaddr(b, at).filter(|a| !(a.ip().is_unspecified() && a.port() == 0))
}

fn pname(b: &[u8], at: usize) -> Option<String> {
    let raw = b.get(at..at + PNAME_LEN)?;
    let end = raw.iter().position(|&c| c == 0).unwrap_or(PNAME_LEN);
    Some(String::from_utf8_lossy(&raw[..end]).into_owned())
}

/// Parse one ntstat message (header + body) from `bytes`.
///
/// `bytes` may carry more than one message; only the first, as sized by its
/// header, is read. A header that claims more bytes than are present, or a
/// body too short for its type, is [`NstatMsg::Other`].
pub fn parse_message(bytes: &[u8]) -> NstatMsg {
    let (Some(ty), Some(len)) = (u32_le(bytes, 8), u16_le(bytes, 12)) else {
        return NstatMsg::Other;
    };
    let len = len as usize;
    if len < HDR_LEN || len > bytes.len() {
        return NstatMsg::Other;
    }
    let msg = &bytes[..len];
    parse_body(msg, ty).unwrap_or(NstatMsg::Other)
}

fn parse_body(msg: &[u8], ty: u32) -> Option<NstatMsg> {
    match ty {
        MSG_SRC_ADDED => Some(NstatMsg::SrcAdded {
            srcref: u64_le(msg, 16)?,
            provider: u32_le(msg, 24)?,
        }),
        MSG_SRC_REMOVED => Some(NstatMsg::SrcRemoved {
            srcref: u64_le(msg, 16)?,
        }),
        MSG_SRC_COUNTS => Some(NstatMsg::SrcCounts {
            srcref: u64_le(msg, 16)?,
            // nstat_counts: rxpackets@32, rxbytes@40, txpackets@48, txbytes@56
            bytes_in: u64_le(msg, 40)?,
            bytes_out: u64_le(msg, 56)?,
        }),
        MSG_SRC_DESC => {
            let srcref = u64_le(msg, 16)?;
            let provider = u32_le(msg, 32)?;
            let desc = msg.get(SRC_DESC_HDR_LEN..)?;
            let obs = match provider {
                PROVIDER_TCP_KERNEL => parse_tcp_descriptor(desc, srcref)?,
                PROVIDER_UDP_KERNEL => parse_udp_descriptor(desc, srcref)?,
                _ => return Some(NstatMsg::Other),
            };
            Some(NstatMsg::SrcDesc(obs))
        }
        _ => Some(NstatMsg::Other),
    }
}

/// Parse a TCP descriptor body into an observation.
pub fn parse_tcp_descriptor(desc: &[u8], srcref: u64) -> Option<NstatObservation> {
    let state = u32_le(desc, TCP_STATE)?;
    Some(NstatObservation {
        srcref,
        transport: Transport::Tcp,
        pid: u32_le(desc, TCP_PID)?,
        pname: pname(desc, TCP_PNAME)?,
        local: sockaddr(desc, TCP_LOCAL),
        remote: remote_addr(desc, TCP_REMOTE),
        state,
        established: (TCPS_ESTABLISHED..=TCPS_TIME_WAIT).contains(&state),
        bytes_in: None,
        bytes_out: None,
    })
}

/// Parse a UDP descriptor body into an observation.
pub fn parse_udp_descriptor(desc: &[u8], srcref: u64) -> Option<NstatObservation> {
    let remote = remote_addr(desc, UDP_REMOTE);
    Some(NstatObservation {
        srcref,
        transport: Transport::Udp,
        pid: u32_le(desc, UDP_PID)?,
        pname: pname(desc, UDP_PNAME)?,
        local: sockaddr(desc, UDP_LOCAL),
        remote,
        state: 0,
        established: remote.is_some(),
        bytes_in: None,
        bytes_out: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> Vec<u8> {
        let path = format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
        let hex = std::fs::read_to_string(path).unwrap();
        let hex = hex.trim();
        (0..hex.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
            .collect()
    }

    #[test]
    fn fixture_header_is_complete() {
        let b = fixture("ntstat_tcp_descriptor.hex");
        assert_eq!(b.len(), 400);
        assert_eq!(u16::from_le_bytes([b[12], b[13]]) as usize, b.len());
        let c = fixture("ntstat_tcp_counts.hex");
        assert_eq!(u16::from_le_bytes([c[12], c[13]]) as usize, c.len());
    }

    #[test]
    fn parses_tcp_descriptor_fixture() {
        let b = fixture("ntstat_tcp_descriptor.hex");
        let NstatMsg::SrcDesc(o) = parse_message(&b) else {
            panic!("not a SrcDesc");
        };
        assert_eq!(o.srcref, 1);
        assert_eq!(o.transport, Transport::Tcp);
        assert_eq!(o.pid, 28329);
        assert_eq!(o.pname, "curl");
        assert_eq!(o.local, Some("10.9.8.151:62856".parse().unwrap()));
        assert_eq!(o.remote, Some("104.20.23.154:80".parse().unwrap()));
        assert_eq!(o.state, 4);
        assert!(o.established);
        assert_eq!(o.bytes_in, None);
        assert_eq!(o.bytes_out, None);
    }

    #[test]
    fn parses_counts_fixture() {
        let b = fixture("ntstat_tcp_counts.hex");
        assert_eq!(
            parse_message(&b),
            NstatMsg::SrcCounts {
                srcref: 111,
                bytes_in: 1547,
                bytes_out: 1272
            }
        );
    }

    #[test]
    fn classifies_added_and_removed() {
        let mut added = vec![0u8; 32];
        added[8..12].copy_from_slice(&MSG_SRC_ADDED.to_le_bytes());
        added[12..14].copy_from_slice(&32u16.to_le_bytes());
        added[16..24].copy_from_slice(&77u64.to_le_bytes());
        added[24..28].copy_from_slice(&PROVIDER_UDP_KERNEL.to_le_bytes());
        assert_eq!(
            parse_message(&added),
            NstatMsg::SrcAdded {
                srcref: 77,
                provider: PROVIDER_UDP_KERNEL
            }
        );
        let mut removed = vec![0u8; 24];
        removed[8..12].copy_from_slice(&MSG_SRC_REMOVED.to_le_bytes());
        removed[12..14].copy_from_slice(&24u16.to_le_bytes());
        removed[16..24].copy_from_slice(&77u64.to_le_bytes());
        assert_eq!(parse_message(&removed), NstatMsg::SrcRemoved { srcref: 77 });
    }

    #[test]
    fn short_or_inconsistent_buffers_are_other() {
        let b = fixture("ntstat_tcp_descriptor.hex");
        assert_eq!(parse_message(&[]), NstatMsg::Other);
        assert_eq!(parse_message(&b[..10]), NstatMsg::Other);
        // Header claims 400 bytes; only 399 present.
        assert_eq!(parse_message(&b[..399]), NstatMsg::Other);
        // Every truncation of the body is rejected, never a panic.
        for n in 0..b.len() {
            assert_eq!(parse_message(&b[..n]), NstatMsg::Other, "len {n}");
        }
        let counts = fixture("ntstat_tcp_counts.hex");
        for n in 0..counts.len() {
            assert_eq!(parse_message(&counts[..n]), NstatMsg::Other, "len {n}");
        }
    }

    #[test]
    fn short_descriptor_is_none() {
        let b = fixture("ntstat_tcp_descriptor.hex");
        let desc = &b[40..];
        assert!(parse_tcp_descriptor(desc, 1).is_some());
        assert!(parse_tcp_descriptor(&desc[..desc.len() - 101], 1).is_none());
        assert!(parse_tcp_descriptor(&[], 1).is_none());
        assert!(parse_udp_descriptor(&[0; 10], 1).is_none());
    }

    #[test]
    fn unspecified_remote_is_none() {
        let b = fixture("ntstat_tcp_descriptor.hex");
        let mut desc = b[40..].to_vec();
        // Zero the remote sockaddr_in (offset 152, 16 bytes) but keep family.
        desc[152 + 2..152 + 8].fill(0);
        let o = parse_tcp_descriptor(&desc, 1).unwrap();
        assert_eq!(o.remote, None);
    }

    /// Constructed (not kernel-captured): the v6 and UDP shapes follow the
    /// same offsets, with `sockaddr_in6` filling the 28-byte union.
    #[test]
    fn parses_constructed_udp_v6_descriptor() {
        let mut d = vec![0u8; 196];
        // local [::1]:5353, remote [2001:db8::1]:53 (sockaddr_in6, len 28, AF_INET6 = 30)
        let put6 = |d: &mut [u8], at: usize, ip: [u8; 16], port: u16| {
            d[at] = 28;
            d[at + 1] = 30;
            d[at + 2..at + 4].copy_from_slice(&port.to_be_bytes());
            d[at + 8..at + 24].copy_from_slice(&ip);
        };
        let mut lo = [0u8; 16];
        lo[15] = 1;
        let mut rem = [0u8; 16];
        rem[0] = 0x20;
        rem[1] = 0x01;
        rem[2] = 0x0d;
        rem[3] = 0xb8;
        rem[15] = 1;
        put6(&mut d, 56, lo, 5353);
        put6(&mut d, 84, rem, 53);
        d[128..132].copy_from_slice(&4242u32.to_le_bytes());
        d[132..132 + 3].copy_from_slice(b"dig");
        let o = parse_udp_descriptor(&d, 9).unwrap();
        assert_eq!(o.srcref, 9);
        assert_eq!(o.transport, Transport::Udp);
        assert_eq!(o.pid, 4242);
        assert_eq!(o.pname, "dig");
        assert_eq!(o.local, Some("[::1]:5353".parse().unwrap()));
        assert_eq!(o.remote, Some("[2001:db8::1]:53".parse().unwrap()));
        assert!(o.established);
        // Unconnected: zero the remote's port and address.
        d[84 + 2..84 + 24].fill(0);
        let o = parse_udp_descriptor(&d, 9).unwrap();
        assert_eq!(o.remote, None);
        assert!(!o.established);
    }
}
