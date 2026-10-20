//! `?host=` on an endpoint only this control plane's own host can serve: a
//! remote host has no transport for it, so asking for one is a 501 that says
//! so rather than a silent local answer.

use crate::error::{ApiError, ApiResult};
use serde::Deserialize;

#[derive(Debug, Default, Deserialize, schemars::JsonSchema)]
pub(crate) struct LocalOnlyQuery {
    /// Must be absent or `local`: the operation runs only on this control
    /// plane's own host.
    #[serde(default)]
    pub(crate) host: Option<String>,
}

impl LocalOnlyQuery {
    /// 501 for any host but `local`, naming `what` as local-only.
    pub(crate) fn require_local(&self, what: &str) -> ApiResult<()> {
        match self.host.as_deref() {
            None | Some("local") => Ok(()),
            Some(host) => Err(ApiError::not_available(format!(
                "{what} is local-only: host {host} has no transport for it"
            ))),
        }
    }
}
