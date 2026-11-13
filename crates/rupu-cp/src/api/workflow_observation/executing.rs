//! One bounded scheduler observation, frozen before deriving response identities.
use super::*;

pub(super) struct Observation {
    pub value: Value,
    pub input_bytes: usize,
    canonical: Vec<u8>,
}

impl Observation {
    pub fn read(owner: &owner::Owner) -> ApiResult<Self> {
        let read = rupu_orchestrator::execution_observation::read_for_owner(
            &owner.input.dir,
            &owner.run,
            owner.archived,
        );
        let canonical = serde_json::to_vec(&read.envelope)
            .map_err(|_| ApiError::internal("workflow execution observation encoding failed"))?;
        if canonical.len() > rupu_orchestrator::execution_observation::MAX_SNAPSHOT_BYTES {
            return Err(too_large("workflow execution observation limit exceeded"));
        }
        let value = serde_json::from_slice(&canonical)
            .map_err(|_| ApiError::internal("workflow execution observation encoding failed"))?;
        Ok(Self {
            value,
            input_bytes: read.execution_input_bytes,
            canonical,
        })
    }

    pub fn bind(&self, input: &mut input::Input) {
        input.bind_execution(&self.canonical);
    }
}
