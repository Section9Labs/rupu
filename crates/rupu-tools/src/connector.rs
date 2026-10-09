//! What the connector tools (`scm.*`, `issues.*`, `github.*`, `gitlab.*`)
//! share: their error type, platform/tracker defaulting, schema generation
//! and the one invoke wrapper that reads the run's SCM registry.
//!
//! Each connector tool takes `rupu_scm::Registry` from
//! [`ToolServices::scm`](crate::ToolServices::scm) (spec W4 D2: the registry
//! already is the port).

use crate::output::ok;
use crate::tool::{ToolContext, ToolError, ToolOutput};
use rupu_scm::{AccountError, IssueTracker, Platform, Registry, ScmError};
use serde_json::Value;
use std::future::Future;
use std::time::Instant;
use thiserror::Error;

/// Why a connector call failed. Its text is the call's error, in every
/// transport (agent loop, `action:` step, `rupu mcp serve`).
#[derive(Debug, Error)]
pub enum ConnectorError {
    /// The connector was reached and failed.
    #[error("tool dispatch failed: {0}")]
    Dispatch(#[from] ScmError),
    /// No account could be picked: ambiguous (`NoRuleMatched`), none
    /// configured (`NoAccounts`), or a bad explicit `account` argument
    /// (`UnknownAccount`). The connector was never reached.
    #[error("account resolution failed: {0}")]
    Account(#[from] AccountError),
    #[error("invalid arguments: {0}")]
    InvalidArgs(String),
}

/// Parse a connector tool's arguments.
pub(crate) fn parse<T: serde::de::DeserializeOwned>(args: Value) -> Result<T, ConnectorError> {
    serde_json::from_value(args).map_err(|e| ConnectorError::InvalidArgs(e.to_string()))
}

/// The JSON Schema of a connector tool's argument struct.
pub(crate) fn schema<T: schemars::JsonSchema>() -> Value {
    serde_json::to_value(schemars::schema_for!(T)).expect("a derived schema serializes")
}

/// Serialize a connector's answer as the call's text.
pub(crate) fn json<T: serde::Serialize>(v: &T) -> String {
    serde_json::to_string(v).expect("connector types serialize")
}

/// Run one connector call against the run's registry. A failure is the
/// call's error ([`ToolError::Execution`] carrying the [`ConnectorError`]
/// text); a run without a registry is never offered these tools (they need
/// [`crate::Service::Scm`]), so its absence here is an execution error too.
pub(crate) async fn invoke<'a, F, Fut>(
    ctx: &'a ToolContext,
    call: F,
) -> Result<ToolOutput, ToolError>
where
    F: FnOnce(&'a Registry) -> Fut,
    Fut: Future<Output = Result<String, ConnectorError>> + 'a,
{
    let started = Instant::now();
    let Some(registry) = ctx.services.scm.as_deref() else {
        return Err(ToolError::Execution(
            "this run has no SCM/issue registry, so connector tools can't run".into(),
        ));
    };
    match call(registry).await {
        Ok(text) => Ok(ok(text).timed(started)),
        Err(e) => Err(ToolError::Execution(e.to_string())),
    }
}

/// An optional platform argument, else `Registry::default_platform()` — the
/// first registered platform with a live connector, honoring
/// `[scm.default]`.
pub(crate) fn resolve_platform(
    arg: Option<&str>,
    reg: &Registry,
) -> Result<Platform, ConnectorError> {
    match arg {
        Some(s) => s.parse::<Platform>().map_err(ConnectorError::InvalidArgs),
        None => reg.default_platform().ok_or_else(|| {
            ConnectorError::InvalidArgs("no platform arg and no [scm.default] configured".into())
        }),
    }
}

/// An optional tracker argument, else `Registry::default_tracker()`
/// (`[issues.default]`).
pub(crate) fn resolve_tracker(
    arg: Option<&str>,
    reg: &Registry,
) -> Result<IssueTracker, ConnectorError> {
    match arg {
        Some(s) => s
            .parse::<IssueTracker>()
            .map_err(ConnectorError::InvalidArgs),
        None => reg.default_tracker().ok_or_else(|| {
            ConnectorError::InvalidArgs("no tracker arg and no [issues.default] configured".into())
        }),
    }
}
