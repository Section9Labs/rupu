use super::Bus;
use crate::{
    address,
    work_queue::{validate_command, validate_context, Store},
    FleetError, OperatorVia, Roster, WorkActor, WorkCommand, WorkContext, WorkError, WorkReply,
};
use std::{
    fs,
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    time::{Duration, Instant},
};

impl Bus {
    /// Runtime-owned actor context must match this exact assembled execution.
    /// Historical receipt replay survives a later roster replacement; new work
    /// requires both the runtime liveness gate and the current strict roster.
    pub fn work_apply(
        &self,
        command: WorkCommand,
        context: WorkContext,
        check_owner: impl FnOnce() -> Result<(), FleetError>,
        check_live: impl FnOnce() -> Result<(), FleetError>,
    ) -> Result<WorkReply, WorkError> {
        validate_context(&context)?;
        validate_command(&command, &context)?;
        let WorkActor::Agent {
            address,
            run_id,
            execution_id,
            agent,
            ..
        } = &context.actor
        else {
            return Err(WorkError::Denied(
                "worker Queue API requires an agent context".into(),
            ));
        };
        if self.participant == address::OPERATOR || address != &self.participant {
            return Err(WorkError::Denied(
                "Queue actor does not match this Bus execution".into(),
            ));
        }
        let _lock = self.work_lock()?;
        check_owner()?;
        if let Some(store) = self.work_receipt_store()? {
            if let Some(reply) = store.replay(&command, &context)? {
                return Ok(reply);
            }
        }
        check_live()?;
        let member = self
            .member()
            .ok_or_else(|| WorkError::Denied("Queue worker must be joined".into()))?;
        if run_id != &member.run_id
            || agent != &member.agent
            || member.work_execution_id.as_ref() != Some(execution_id)
        {
            return Err(WorkError::Denied(
                "Queue actor does not match this Bus execution".into(),
            ));
        }
        let roster = Roster::read_work_authority(&self.participants_path())?;
        if !roster.get(address).is_some_and(|p| {
            p.live
                && p.member.run_id == *run_id
                && p.member.agent == *agent
                && p.member.work_execution_id.as_ref() == Some(execution_id)
        }) {
            return Err(WorkError::Denied(
                "Queue execution is no longer the current participant".into(),
            ));
        }
        Store::open(self.space(), true)?
            .ok_or_else(|| WorkError::Persistence("Queue database unavailable".into()))?
            .apply(&command, &context)
    }

    pub fn operator_work_apply(
        &self,
        command: WorkCommand,
        via: OperatorVia,
        check_owner: impl FnOnce() -> Result<(), FleetError>,
        check_live: impl FnOnce() -> Result<(), FleetError>,
    ) -> Result<WorkReply, WorkError> {
        if self.participant != address::OPERATOR || self.is_joined() {
            return Err(WorkError::Denied(
                "operator Queue API requires the reserved unjoined operator".into(),
            ));
        }
        let context = WorkContext {
            actor: WorkActor::Operator { via },
            source: None,
        };
        validate_context(&context)?;
        validate_command(&command, &context)?;
        let _lock = self.work_lock()?;
        check_owner()?;
        if let Some(store) = self.work_receipt_store()? {
            if let Some(reply) = store.replay(&command, &context)? {
                return Ok(reply);
            }
        }
        check_live()?;
        Store::open(self.space(), true)?
            .ok_or_else(|| WorkError::Persistence("Queue database unavailable".into()))?
            .apply(&command, &context)
    }

    fn work_receipt_store(&self) -> Result<Option<Store>, WorkError> {
        match Store::open(self.space(), false) {
            Err(WorkError::ReadOnly(_)) => Store::recover_existing(self.space()),
            other => other,
        }
    }

    /// Same lock inode/order as join/leave, with bounded waiting and safe opens.
    /// The enclosing runtime/CP retains the owner's lifecycle lease.
    fn work_lock(&self) -> Result<fs::File, WorkError> {
        if let Ok(meta) = fs::symlink_metadata(self.space()) {
            if !meta.is_dir() {
                return Err(WorkError::Denied(
                    "Queue message space is redirected".into(),
                ));
            }
        } else {
            fs::create_dir_all(self.space()).map_err(|e| WorkError::Persistence(e.to_string()))?;
        }
        let path = self.space().join("log.jsonl.lock");
        let file = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(
                (rustix::fs::OFlags::NOFOLLOW | rustix::fs::OFlags::NONBLOCK).bits() as i32,
            )
            .open(&path)
            .map_err(|e| WorkError::Denied(format!("Queue lock unavailable: {e}")))?;
        let meta = file
            .metadata()
            .map_err(|e| WorkError::Persistence(e.to_string()))?;
        if !meta.is_file() || meta.nlink() != 1 || meta.len() != 0 {
            return Err(WorkError::Denied(
                "Queue lock is not a regular owned lock file".into(),
            ));
        }
        let deadline = Instant::now() + Duration::from_millis(250);
        loop {
            match rustix::fs::flock(&file, rustix::fs::FlockOperation::NonBlockingLockExclusive) {
                Ok(()) => break,
                Err(rustix::io::Errno::INTR) => continue,
                Err(rustix::io::Errno::WOULDBLOCK) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(5))
                }
                Err(rustix::io::Errno::WOULDBLOCK) => return Err(WorkError::Busy),
                Err(e) => return Err(WorkError::Persistence(e.to_string())),
            }
        }
        let named = fs::symlink_metadata(&path).map_err(|_| WorkError::OwnerChanged)?;
        if !named.is_file() || named.dev() != meta.dev() || named.ino() != meta.ino() {
            return Err(WorkError::OwnerChanged);
        }
        Ok(file)
    }
}
