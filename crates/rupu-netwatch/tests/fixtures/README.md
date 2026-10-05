# Real `sock_diag` fixtures

Captured from a live Linux kernel (Kali arm64, kernel 6.19) on 2026-10-04 via a
`SOCK_DIAG_BY_FAMILY` TCP dump over `NETLINK_SOCK_DIAG`. Each file is the hex of
one `nlmsghdr` + `inet_diag_msg` (+ attributes) for a single socket — exactly the
bytes the Linux backend's parser must decode. They are real kernel output, not
synthesized, so the parser is tested against ground truth.

- `sock_diag_tcp_listener.hex` — a listening socket (remote addr all-zero).
- `sock_diag_tcp_established.hex` — an ESTABLISHED socket with a real remote.
