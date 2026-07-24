//! Small networking helpers shared by the CP server and the CLI enroll flow.

#![deny(clippy::all)]

use std::net::{IpAddr, UdpSocket};

/// Best-effort detection of this machine's routable (non-loopback) IP.
///
/// Uses the classic UDP-connect trick: bind an ephemeral UDP socket and
/// `connect` it to a public address, then read the socket's local address.
/// No packet is ever sent — `connect` on a UDP datagram socket only records
/// the destination and consults the kernel routing table — so this works
/// offline and never blocks.
///
/// Returns `None` on any error (no usable interface, sandboxed network,
/// etc.); callers fall back to a placeholder host.
pub fn detect_routable_ip() -> Option<IpAddr> {
    let sock = UdpSocket::bind("0.0.0.0:0").ok()?;
    sock.connect("8.8.8.8:80").ok()?;
    let ip = sock.local_addr().ok()?.ip();
    // An unspecified or loopback result means the routing table had no
    // usable outbound interface — treat as "not detected".
    if ip.is_unspecified() || ip.is_loopback() {
        return None;
    }
    Some(ip)
}
