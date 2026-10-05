# Real `sock_diag` fixtures

Captured from a live Linux kernel (Kali arm64, kernel 6.19) on 2026-10-04 via a
`SOCK_DIAG_BY_FAMILY` TCP dump over `NETLINK_SOCK_DIAG`. Each file is the hex of
one `nlmsghdr` + `inet_diag_msg` (+ attributes) for a single socket — exactly the
bytes the Linux backend's parser must decode. They are real kernel output, not
synthesized, so the parser is tested against ground truth.

- `sock_diag_tcp_listener.hex` — a listening socket (remote addr all-zero).
- `sock_diag_tcp_established.hex` — an ESTABLISHED socket with a real remote.

# Real `ntstat` fixtures (macOS)

Captured on a live Mac (Darwin 27, arm64) on 2026-10-04 from the
`com.apple.network.statistics` kernel control (`PF_SYSTEM`/`SYSPROTO_CONTROL`),
subscribed to the TCP provider. Each file is the hex of one whole message
(`nstat_msg_hdr` + body); the header's `length` field equals the byte count.
They are real kernel output, not synthesized.

- `ntstat_tcp_descriptor.hex` — a `SRC_DESC` (10003) for an ESTABLISHED TCP
  socket owned by a `curl` process (400 bytes). Carries pid/pname/addrs/state
  but no byte counters.
- `ntstat_tcp_counts.hex` — a `SRC_COUNTS` (10004) answering `QUERY_SRC` for a
  live socket (144 bytes): the only place the kernel reports rx/tx bytes.
