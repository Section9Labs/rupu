//! Owner-bound worker side of the private broker channel. Cancellation closes
//! the actual socket even while other Arc handles survive: a pending request
//! cannot silently remain eligible, and a consumed permit cannot be replayed.
#![cfg(unix)]
use super::protocol::{read_frame, write_frame, ClientFrame, ServerFrame};
use async_trait::async_trait;
use rupu_tools::approval::{
    ApprovalCompletion, ApprovalError, ApprovalObserver, ApprovalOutcome, ApprovalOwner,
    ApprovalPermit, CallApprovalRequest, CallAuthorizer,
};
use std::{
    net::Shutdown,
    os::unix::net::UnixStream,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};
use tokio::sync::{Mutex, OwnedMutexGuard};

pub struct Connection {
    stream: Arc<Mutex<tokio::net::UnixStream>>,
    shutdown: UnixStream,
    closed: AtomicBool,
    session_id: String,
    pub instance_id: String,
    pub broker_epoch: String,
    pub worker_generation: u64,
}
impl std::fmt::Debug for Connection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ApprovalConnection")
            .field("closed", &self.closed.load(Ordering::Acquire))
            .finish_non_exhaustive()
    }
}
impl Connection {
    pub async fn bind(stream: UnixStream, session_id: String) -> Result<Arc<Self>, ApprovalError> {
        stream.set_nonblocking(true).map_err(error)?;
        let shutdown = stream.try_clone().map_err(error)?;
        let mut stream = tokio::net::UnixStream::from_std(stream).map_err(error)?;
        let result = tokio::time::timeout(Duration::from_secs(5), async {
            write_frame(
                &mut stream,
                &ClientFrame::Bind {
                    session_id: session_id.clone(),
                    worker_pid: std::process::id(),
                },
            )
            .await
            .map_err(error)?;
            match read_frame(&mut stream).await.map_err(error)? {
                ServerFrame::Bound {
                    instance_id,
                    broker_epoch,
                    worker_generation,
                } if !instance_id.is_empty()
                    && !broker_epoch.is_empty()
                    && worker_generation > 0 =>
                {
                    Ok((instance_id, broker_epoch, worker_generation))
                }
                _ => Err(ApprovalError("broker registration refused".into())),
            }
        })
        .await
        .map_err(|_| ApprovalError("broker registration timed out".into()))?;
        let (instance_id, broker_epoch, worker_generation) = result?;
        Ok(Arc::new(Self {
            stream: Arc::new(Mutex::new(stream)),
            shutdown,
            closed: AtomicBool::new(false),
            session_id,
            instance_id,
            broker_epoch,
            worker_generation,
        }))
    }
    pub fn authorizer(
        self: &Arc<Self>,
        owner: ApprovalOwner,
    ) -> Result<Arc<dyn CallAuthorizer>, ApprovalError> {
        if self.closed.load(Ordering::Acquire)
            || owner.session_id != self.session_id
            || owner.root_turn_run_id != owner.run_id
            || owner.parent_run_id.is_some()
        {
            return Err(ApprovalError("worker approval ownership changed".into()));
        }
        Ok(Arc::new(Authorizer {
            connection: self.clone(),
            owner,
        }))
    }
    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Acquire)
    }
    fn close(&self) {
        self.closed.store(true, Ordering::Release);
        let _ = self.shutdown.shutdown(Shutdown::Both);
    }
    async fn lock(
        self: &Arc<Self>,
    ) -> Result<OwnedMutexGuard<tokio::net::UnixStream>, ApprovalError> {
        if self.closed.load(Ordering::Acquire) {
            return Err(ApprovalError("broker channel closed".into()));
        }
        let guard = self.stream.clone().lock_owned().await;
        if self.closed.load(Ordering::Acquire) {
            return Err(ApprovalError("broker channel closed".into()));
        }
        Ok(guard)
    }
}
fn error(_: impl std::fmt::Display) -> ApprovalError {
    ApprovalError("private broker channel failed".into())
}
struct InFlight {
    connection: Arc<Connection>,
    armed: bool,
}
impl InFlight {
    fn new(connection: Arc<Connection>) -> Self {
        Self {
            connection,
            armed: true,
        }
    }
}
impl Drop for InFlight {
    fn drop(&mut self) {
        if self.armed {
            self.connection.close();
        }
    }
}
struct Authorizer {
    connection: Arc<Connection>,
    owner: ApprovalOwner,
}
#[async_trait]
impl CallAuthorizer for Authorizer {
    fn root_owner(&self) -> ApprovalOwner {
        self.owner.clone()
    }
    async fn check_owner(&self, owner: &ApprovalOwner) -> Result<(), ApprovalError> {
        if owner.session_id != self.owner.session_id
            || owner.root_turn_run_id != self.owner.root_turn_run_id
            || owner.workspace_id != self.owner.workspace_id
            || owner.workspace_path != self.owner.workspace_path
        {
            return Err(ApprovalError("approval scope mismatch".into()));
        }
        let mut stream = self.connection.lock().await?;
        let mut in_flight = InFlight::new(self.connection.clone());
        tokio::time::timeout(Duration::from_secs(5), async {
            write_frame(
                &mut *stream,
                &ClientFrame::CheckOwner {
                    owner: owner.clone(),
                },
            )
            .await
            .map_err(error)?;
            match read_frame(&mut *stream).await.map_err(error)? {
                ServerFrame::Ready {} => Ok(()),
                _ => Err(ApprovalError("approval owner no longer current".into())),
            }
        })
        .await
        .map_err(|_| ApprovalError("approval owner check timed out".into()))??;
        in_flight.armed = false;
        Ok(())
    }
    async fn authorize(
        &self,
        request: CallApprovalRequest,
        observer: &mut ApprovalObserver<'_>,
    ) -> Result<ApprovalOutcome, ApprovalError> {
        if request.owner.session_id != self.owner.session_id
            || request.owner.root_turn_run_id != self.owner.root_turn_run_id
            || request.owner.workspace_id != self.owner.workspace_id
            || request.owner.workspace_path != self.owner.workspace_path
        {
            return Err(ApprovalError("approval request scope mismatch".into()));
        }
        let mut stream = self.connection.lock().await?;
        let mut in_flight = InFlight::new(self.connection.clone());
        let response = tokio::time::timeout(Duration::from_secs(660), async {
            write_frame(
                &mut *stream,
                &ClientFrame::Request {
                    call: request.clone(),
                },
            )
            .await
            .map_err(error)?;
            let mut pending: Option<(String, String)> = None;
            loop {
                match read_frame(&mut *stream).await.map_err(error)? {
                    ServerFrame::Pending { event } => {
                        if pending.is_some()
                            || event["session_id"].as_str()
                                != Some(request.owner.session_id.as_str())
                            || event["run_id"].as_str() != Some(request.owner.run_id.as_str())
                            || event["root_turn_run_id"].as_str()
                                != Some(request.owner.root_turn_run_id.as_str())
                            || event["workspace_id"].as_str()
                                != Some(request.owner.workspace_id.as_str())
                            || event["tool_call_id"].as_str() != Some(request.tool_call_id.as_str())
                            || event["turn_index"].as_u64() != Some(request.turn_index)
                            || event["call_sequence"].as_u64() != Some(request.call_sequence)
                            || event["worker_generation"].as_u64()
                                != Some(self.connection.worker_generation)
                            || event["root_run_id"].as_str()
                                != Some(request.owner.root_run_id.as_str())
                            || event.get("parent_run_id")
                                != Some(&serde_json::json!(request.owner.parent_run_id))
                            || event["tool"].as_str() != Some(request.tool.as_str())
                            || event.get("effect") != Some(&serde_json::json!(request.effect))
                            || event["cp_instance_id"].as_str()
                                != Some(self.connection.instance_id.as_str())
                            || event["broker_epoch"].as_str()
                                != Some(self.connection.broker_epoch.as_str())
                        {
                            return Err(ApprovalError("approval event ownership mismatch".into()));
                        }
                        let id = event["approval_id"]
                            .as_str()
                            .filter(|s| !s.is_empty())
                            .ok_or_else(|| ApprovalError("missing approval identity".into()))?
                            .to_owned();
                        let fingerprint = event["intent_fingerprint"]
                            .as_str()
                            .filter(|s| !s.is_empty())
                            .ok_or_else(|| ApprovalError("missing approval fingerprint".into()))?
                            .to_owned();
                        observer(event)?;
                        pending = Some((id, fingerprint));
                    }
                    ServerFrame::Reserved {
                        approval_id,
                        intent_fingerprint,
                    } if pending.as_ref()
                        == Some(&(approval_id.clone(), intent_fingerprint.clone())) =>
                    {
                        return Ok(Ok((approval_id, intent_fingerprint)))
                    }
                    ServerFrame::Denied { reason } => return Ok(Err(reason)),
                    _ => return Err(ApprovalError("unexpected approval broker reply".into())),
                }
            }
        })
        .await
        .map_err(|_| ApprovalError("approval wait expired".into()))??;
        match response {
            Ok((approval_id, intent_fingerprint)) => {
                Ok(ApprovalOutcome::Permitted(Box::new(Permit {
                    approval_id,
                    intent_fingerprint,
                    stream: Some(stream),
                    guard: in_flight,
                })))
            }
            Err(reason) => {
                in_flight.armed = false;
                Ok(ApprovalOutcome::Denied { reason })
            }
        }
    }
}
struct Permit {
    approval_id: String,
    intent_fingerprint: String,
    // Drop the armed guard before releasing the serial lock.
    guard: InFlight,
    stream: Option<OwnedMutexGuard<tokio::net::UnixStream>>,
}
#[async_trait]
impl ApprovalPermit for Permit {
    fn approval_id(&self) -> &str {
        &self.approval_id
    }
    fn intent_fingerprint(&self) -> &str {
        &self.intent_fingerprint
    }
    async fn complete(
        mut self: Box<Self>,
        outcome: ApprovalCompletion,
    ) -> Result<(), ApprovalError> {
        let stream = self
            .stream
            .as_mut()
            .ok_or_else(|| ApprovalError("approval already consumed".into()))?;
        tokio::time::timeout(Duration::from_secs(5), async {
            write_frame(
                &mut **stream,
                &ClientFrame::Complete {
                    approval_id: self.approval_id.clone(),
                    intent_fingerprint: self.intent_fingerprint.clone(),
                    outcome,
                },
            )
            .await
            .map_err(error)?;
            match read_frame(&mut **stream).await.map_err(error)? {
                ServerFrame::Recorded {} => Ok(()),
                _ => Err(ApprovalError("approval completion unconfirmed".into())),
            }
        })
        .await
        .map_err(|_| ApprovalError("approval completion timed out".into()))??;
        self.guard.armed = false;
        Ok(())
    }
}

#[cfg(test)]
#[path = "client_tests.rs"]
mod tests;
