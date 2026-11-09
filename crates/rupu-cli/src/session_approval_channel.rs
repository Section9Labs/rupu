//! Private stdin socket handoff used only by authenticated CP session launches.
//! The socket is detached from FD0 immediately and CLOEXEC on every owned copy,
//! so normal tool subprocesses cannot inherit approval authority.
#![cfg(unix)]
use rupu_runtime::session_approvals::client::Connection;
use std::{
    os::{fd::OwnedFd, unix::net::UnixStream},
    sync::{Arc, Mutex, OnceLock},
};

static START: Mutex<Option<UnixStream>> = Mutex::new(None);
static WORKER: OnceLock<Arc<Connection>> = OnceLock::new();

fn take_stdin() -> anyhow::Result<UnixStream> {
    let fd = rustix::io::fcntl_dupfd_cloexec(std::io::stdin(), 3)?;
    let null = std::fs::File::open("/dev/null")?;
    rustix::stdio::dup2_stdin(&null)?;
    let stream = UnixStream::from(fd);
    stream.peer_addr()?;
    Ok(stream)
}
pub fn accept_start() -> anyhow::Result<()> {
    let stream = take_stdin()?;
    let mut slot = START
        .lock()
        .map_err(|_| anyhow::anyhow!("approval handoff lock failed"))?;
    anyhow::ensure!(slot.is_none(), "approval handoff already occupied");
    *slot = Some(stream);
    Ok(())
}
pub fn has_start() -> bool {
    START.lock().map(|v| v.is_some()).unwrap_or(false)
}
pub fn take_start() -> anyhow::Result<Option<UnixStream>> {
    Ok(START
        .lock()
        .map_err(|_| anyhow::anyhow!("approval handoff lock failed"))?
        .take())
}
pub async fn bind_worker(session_id: &str) -> anyhow::Result<()> {
    let stream = take_start()?.ok_or_else(|| anyhow::anyhow!("approval worker handoff missing"))?;
    let connection = Connection::bind(stream, session_id.to_owned()).await?;
    WORKER
        .set(connection)
        .map_err(|_| anyhow::anyhow!("approval worker already bound"))
}
pub fn authorizer(
    owner: rupu_tools::approval::ApprovalOwner,
) -> anyhow::Result<Arc<dyn rupu_tools::approval::CallAuthorizer>> {
    WORKER
        .get()
        .ok_or_else(|| anyhow::anyhow!("Ask session has no private approval channel"))?
        .authorizer(owner)
        .map_err(Into::into)
}
pub fn stdin_for_child(stream: UnixStream) -> std::process::Stdio {
    std::process::Stdio::from(OwnedFd::from(stream))
}

/// A broken private capability is never healed in-place. The worker exits
/// after preserving its current turn, so a later explicit send can rebind.
pub fn worker_is_usable() -> bool {
    WORKER
        .get()
        .is_some_and(|connection| !connection.is_closed())
}
