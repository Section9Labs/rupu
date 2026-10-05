//! Linux capture backend. The modules here are pure (no syscalls, no
//! `cfg`): they decode and build netlink bytes and make decisions over plain
//! inputs, so they compile and are tested on every platform. The socket and
//! cgroup filesystem operations live in later, `cfg(target_os = "linux")`
//! modules.

pub mod cgroup;
#[cfg(target_os = "linux")]
pub mod netlink;
pub mod parse;
pub mod req;
