//! macOS capture backend. `parse` is pure (no syscalls, no `cfg`): it decodes
//! `ntstat` kernel-control messages from bytes and is tested on every
//! platform. The socket, process-tree and watcher code lives in
//! `cfg(target_os = "macos")` modules.

pub mod parse;

#[cfg(target_os = "macos")]
pub mod ntstat;
#[cfg(target_os = "macos")]
pub mod proctree;
#[cfg(target_os = "macos")]
pub mod watch;
