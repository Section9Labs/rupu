//! Pure decoder for `NETLINK_SOCK_DIAG` replies.
//!
//! Everything here reads `&[u8]` only: no sockets, no filesystem, no `cfg`.
//! It is tested against real kernel output in `tests/fixtures/`.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

use crate::types::Transport;

/// `INET_DIAG_INFO`: a `tcp_info` struct.
const INET_DIAG_INFO: u16 = 2;
/// `INET_DIAG_CGROUP`: the socket's cgroup v2 id (u64).
const INET_DIAG_CGROUP: u16 = 21;

/// Size of `nlmsghdr`.
const NLMSG_HDR_LEN: usize = 16;
/// `inet_diag_msg` fixed part: family/state/timer/retrans (4) + sockid (48)
/// + expires/rqueue/wqueue/uid/inode (20).
const INET_DIAG_MSG_LEN: usize = 72;

/// Offsets inside `tcp_info` (stable since Linux 4.1).
const TCPI_BYTES_ACKED: usize = 120;
const TCPI_BYTES_RECEIVED: usize = 128;

/// One socket as reported by an `inet_diag` dump or destroy event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InetDiagObservation {
    /// `idiag_cookie` (two little-endian u32 joined).
    pub socket_id: u64,
    /// Which dump this came from.
    pub transport: Transport,
    /// `idiag_family == AF_INET`.
    pub family_v4: bool,
    /// `idiag_state` (1 = ESTABLISHED, 10 = LISTEN, ...).
    pub state: u8,
    /// Local end.
    pub local: Option<SocketAddr>,
    /// Remote end; `None` for an all-zero address with port 0.
    pub remote: Option<SocketAddr>,
    /// `INET_DIAG_CGROUP`, when present.
    pub cgroup_id: Option<u64>,
    /// `tcpi_bytes_received` from `INET_DIAG_INFO`, when present.
    pub bytes_in: Option<u64>,
    /// `tcpi_bytes_acked` from `INET_DIAG_INFO`, when present.
    pub bytes_out: Option<u64>,
    /// The socket reached ESTABLISHED or a later connected state.
    pub established: bool,
}

fn u16_le(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(at..at + 2)?.try_into().ok()?))
}

fn u32_le(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

fn u64_le(b: &[u8], at: usize) -> Option<u64> {
    Some(u64::from_le_bytes(b.get(at..at + 8)?.try_into().ok()?))
}

fn addr(raw: &[u8], v4: bool, port: u16) -> SocketAddr {
    let ip = if v4 {
        IpAddr::V4(Ipv4Addr::new(raw[0], raw[1], raw[2], raw[3]))
    } else {
        let mut o = [0u8; 16];
        o.copy_from_slice(&raw[..16]);
        IpAddr::V6(Ipv6Addr::from(o))
    };
    SocketAddr::new(ip, port)
}

/// States a connected socket passes through once established (TCP), or
/// `ESTABLISHED` alone for a connected UDP socket.
fn is_established(transport: Transport, state: u8) -> bool {
    match transport {
        // ESTABLISHED, FIN_WAIT1, FIN_WAIT2, TIME_WAIT, CLOSE_WAIT,
        // LAST_ACK, CLOSING. Not SYN_*, CLOSE or LISTEN.
        Transport::Tcp => matches!(state, 1 | 4 | 5 | 6 | 8 | 9 | 11),
        Transport::Udp => state == 1,
    }
}

/// Parse one `inet_diag_msg` (the bytes after the 16-byte `nlmsghdr`) from a
/// dump of `transport`. `None` when it is too short to hold the fixed part.
pub fn parse_inet_diag(body: &[u8], transport: Transport) -> Option<InetDiagObservation> {
    if body.len() < INET_DIAG_MSG_LEN {
        return None;
    }
    let family_v4 = body[0] == 2; // AF_INET
    let state = body[1];
    let sport = u16::from_be_bytes([body[4], body[5]]);
    let dport = u16::from_be_bytes([body[6], body[7]]);
    let local = addr(&body[8..24], family_v4, sport);
    let remote_raw = addr(&body[24..40], family_v4, dport);
    let remote = if remote_raw.ip().is_unspecified() && dport == 0 {
        None
    } else {
        Some(remote_raw)
    };
    let lo = u32_le(body, 44)? as u64;
    let hi = u32_le(body, 48)? as u64;
    let socket_id = lo | (hi << 32);

    let mut cgroup_id = None;
    let mut bytes_in = None;
    let mut bytes_out = None;
    let mut off = INET_DIAG_MSG_LEN;
    while let (Some(len), Some(ty)) = (u16_le(body, off), u16_le(body, off + 2)) {
        let len = len as usize;
        if len < 4 || off + len > body.len() {
            break;
        }
        let payload = &body[off + 4..off + len];
        match ty {
            INET_DIAG_CGROUP => cgroup_id = u64_le(payload, 0),
            INET_DIAG_INFO => {
                bytes_out = u64_le(payload, TCPI_BYTES_ACKED);
                bytes_in = u64_le(payload, TCPI_BYTES_RECEIVED);
            }
            _ => {}
        }
        off += (len + 3) & !3;
    }

    Some(InetDiagObservation {
        socket_id,
        transport,
        family_v4,
        state,
        local: Some(local),
        remote,
        cgroup_id,
        bytes_in,
        bytes_out,
        established: is_established(transport, state),
    })
}

/// Iterate the netlink messages in a receive buffer, yielding
/// `(nlmsg_type, body)` where `body` follows the 16-byte header. Stops at a
/// truncated or malformed record.
pub fn nlmsgs(buf: &[u8]) -> impl Iterator<Item = (u16, &[u8])> {
    let mut rest = buf;
    std::iter::from_fn(move || {
        let len = u32_le(rest, 0)? as usize;
        let ty = u16_le(rest, 4)?;
        if len < NLMSG_HDR_LEN || len > rest.len() {
            return None;
        }
        let body = &rest[NLMSG_HDR_LEN..len];
        let advance = ((len + 3) & !3).min(rest.len());
        rest = &rest[advance..];
        Some((ty, body))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn load(name: &str) -> Vec<u8> {
        let path = format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
        let hex = std::fs::read_to_string(path).unwrap();
        let hex = hex.trim();
        (0..hex.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
            .collect()
    }

    #[test]
    fn parses_established_fixture_with_remote_and_cookie() {
        let msg = load("sock_diag_tcp_established.hex");
        let obs = parse_inet_diag(&msg[16..], Transport::Tcp).unwrap();
        assert_eq!(obs.state, 1);
        assert!(obs.family_v4);
        assert!(obs.established);
        assert_eq!(obs.transport, Transport::Tcp);
        assert_eq!(obs.local, Some("10.9.10.152:24007".parse().unwrap()));
        assert_eq!(obs.remote, Some("10.9.10.152:49145".parse().unwrap()));
        assert_eq!(obs.socket_id, 0x4007);
        assert_eq!(obs.cgroup_id, Some(0x307b));
        assert_eq!(obs.bytes_out, Some(31060));
        assert_eq!(obs.bytes_in, Some(1784));
    }

    #[test]
    fn parses_listener_fixture_without_remote() {
        let msg = load("sock_diag_tcp_listener.hex");
        let obs = parse_inet_diag(&msg[16..], Transport::Tcp).unwrap();
        assert_eq!(obs.state, 10);
        assert!(obs.family_v4);
        assert!(!obs.established);
        assert_eq!(obs.local, Some("0.0.0.0:22".parse().unwrap()));
        assert_eq!(obs.remote, None);
        assert_eq!(obs.socket_id, 1);
        assert_eq!(obs.cgroup_id, Some(0xaa7));
    }

    #[test]
    fn bad_rtattr_lengths_are_tolerated() {
        let msg = load("sock_diag_tcp_established.hex");
        let fixed = &msg[16..16 + INET_DIAG_MSG_LEN];

        // rta_len < 4 ends attribute parsing.
        let mut body = fixed.to_vec();
        body.extend_from_slice(&[2, 0, 21, 0, 0, 0, 0, 0]);
        let obs = parse_inet_diag(&body, Transport::Tcp).unwrap();
        assert_eq!(
            (obs.cgroup_id, obs.bytes_in, obs.bytes_out),
            (None, None, None)
        );

        // rta_len overrunning the buffer ends attribute parsing.
        let mut body = fixed.to_vec();
        body.extend_from_slice(&[0xff, 0x00, 21, 0, 1, 2, 3, 4]);
        let obs = parse_inet_diag(&body, Transport::Tcp).unwrap();
        assert_eq!(
            (obs.cgroup_id, obs.bytes_in, obs.bytes_out),
            (None, None, None)
        );
        assert_eq!(obs.state, 1);

        // A trailing 1-3 byte fragment is ignored.
        let mut body = fixed.to_vec();
        body.extend_from_slice(&[9, 0]);
        assert!(parse_inet_diag(&body, Transport::Tcp).is_some());
    }

    #[test]
    fn malformed_short_buffer_is_none() {
        assert!(parse_inet_diag(&[], Transport::Tcp).is_none());
        assert!(parse_inet_diag(&[0u8; 71], Transport::Tcp).is_none());
    }

    #[test]
    fn short_info_attr_leaves_bytes_none() {
        let msg = load("sock_diag_tcp_established.hex");
        let mut body = msg[16..].to_vec();
        // Truncate INET_DIAG_INFO (the last attribute) to 40 bytes of payload.
        let len = body.len();
        let info_start = len - 284;
        body.truncate(info_start + 4 + 40);
        body[info_start..info_start + 2].copy_from_slice(&44u16.to_le_bytes());
        let obs = parse_inet_diag(&body, Transport::Tcp).unwrap();
        assert_eq!(obs.bytes_in, None);
        assert_eq!(obs.bytes_out, None);
        assert_eq!(obs.cgroup_id, Some(0x307b));
    }

    #[test]
    fn nlmsgs_yields_each_record_and_stops_on_truncation() {
        let msg = load("sock_diag_tcp_established.hex");
        assert_eq!(msg.len(), 400);
        let mut two = msg.clone();
        two.extend_from_slice(&msg);
        let all: Vec<_> = nlmsgs(&two).collect();
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].0, 20);
        assert_eq!(all[0].1, &msg[16..]);
        // Cut the second record short: only the first survives.
        assert_eq!(nlmsgs(&two[..790]).count(), 1);
        assert_eq!(nlmsgs(&[1, 0, 0]).count(), 0);
    }
}
