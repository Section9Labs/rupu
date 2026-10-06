//! Pure `sock_diag` request builder: bytes only, no socket.

/// `SOCK_DIAG_BY_FAMILY`.
const SOCK_DIAG_BY_FAMILY: u16 = 20;
/// `NLM_F_REQUEST | NLM_F_DUMP`.
const NLM_FLAGS_DUMP: u16 = 0x0001 | 0x0300;
/// `1 << (INET_DIAG_INFO - 1)`: ask for the `tcp_info` attribute.
const EXT_INFO: u8 = 1 << (2 - 1);
// INET_DIAG_CGROUP_ID (21) needs no ext bit: the kernel attaches it to every
// dump reply on its own (confirmed by the real fixtures).
/// Size of `nlmsghdr`.
const NLMSG_HDR_LEN: usize = 16;
/// `inet_diag_req_v2`: family, protocol, ext, pad, states, sockid (48).
const REQ_LEN: usize = 56;

/// Multicast groups for socket-destroy events: `SKNLGRP_INET_TCP_DESTROY`,
/// `SKNLGRP_INET_UDP_DESTROY`, `SKNLGRP_INET6_TCP_DESTROY`,
/// `SKNLGRP_INET6_UDP_DESTROY`, as the bitmask `bind` takes.
pub const DESTROY_GROUPS: u32 = (1 << 0) | (1 << 1) | (1 << 2) | (1 << 3);

/// Bytes of a `SOCK_DIAG_BY_FAMILY` dump request for `(family, proto)`,
/// asking for `INET_DIAG_INFO` on sockets in every state. The kernel echoes
/// `seq` in each reply.
pub fn dump_request(family: u8, proto: u8, seq: u32) -> Vec<u8> {
    let total = NLMSG_HDR_LEN + REQ_LEN;
    let mut b = Vec::with_capacity(total);
    b.extend_from_slice(&(total as u32).to_le_bytes());
    b.extend_from_slice(&SOCK_DIAG_BY_FAMILY.to_le_bytes());
    b.extend_from_slice(&NLM_FLAGS_DUMP.to_le_bytes());
    b.extend_from_slice(&seq.to_le_bytes());
    b.extend_from_slice(&0u32.to_le_bytes()); // nlmsg_pid
    b.push(family); // sdiag_family
    b.push(proto); // sdiag_protocol
    b.push(EXT_INFO); // idiag_ext
    b.push(0); // pad
    b.extend_from_slice(&0xffff_ffffu32.to_le_bytes()); // idiag_states
    b.extend_from_slice(&[0u8; 48]); // idiag_sockid (wildcard)
    debug_assert_eq!(b.len(), total);
    b
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dump_request_layout() {
        let r = dump_request(2, 6, 1);
        assert_eq!(r.len(), 72);
        assert_eq!(u32::from_le_bytes(r[0..4].try_into().unwrap()), 72);
        assert_eq!(u16::from_le_bytes(r[4..6].try_into().unwrap()), 20);
        assert_eq!(u16::from_le_bytes(r[6..8].try_into().unwrap()), 0x301);
        assert_eq!(u32::from_le_bytes(r[8..12].try_into().unwrap()), 1);
        assert_eq!(u32::from_le_bytes(r[12..16].try_into().unwrap()), 0);
        assert_eq!(r[16], 2);
        assert_eq!(r[17], 6);
        assert_eq!(r[18] & (1 << 1), 1 << 1, "INET_DIAG_INFO requested");
        assert_eq!(
            u32::from_le_bytes(r[20..24].try_into().unwrap()),
            0xffff_ffff
        );
        assert!(r[24..].iter().all(|&x| x == 0));
    }

    #[test]
    fn dump_request_echoes_seq_family_proto() {
        let r = dump_request(10, 17, 0xdead_beef);
        assert_eq!(
            u32::from_le_bytes(r[8..12].try_into().unwrap()),
            0xdead_beef
        );
        assert_eq!((r[16], r[17]), (10, 17));
    }

    #[test]
    fn destroy_groups_cover_tcp_udp_v4_v6() {
        assert_eq!(DESTROY_GROUPS, 0b1111);
    }
}
