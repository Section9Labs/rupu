//! The one permission policy. Every tool call — agent loop, sub-agent, MCP
//! dispatch — is decided by [`PermissionPolicy::decide`] from the tool's
//! declared [`Effect`] and the run's [`PermissionMode`]. No permission code
//! reads a tool's name.
//!
//! | Effect ↓ / Mode → | bypass | ask + prompter | ask, no prompter | readonly |
//! |---|---|---|---|---|
//! | Read | allow | allow | allow | allow |
//! | Record | allow | allow | allow | allow |
//! | Write | allow | prompt | allow, degraded | deny |
//! | External | allow | prompt | allow, degraded | deny |
//! | Spawn | allow, ceiling bypass | allow, ceiling ask | allow, ceiling ask | allow, ceiling readonly |
//!
//! A child run's mode is `min(ceiling, child's own permissionMode)`
//! ([`PermissionPolicy::ceiling_for_child`]), so a child never has more than
//! its parent. Mode resolution (CLI flag > agent frontmatter > config >
//! default `Ask`) happens upstream; [`PermissionMode::parse`] is the one
//! parser of the mode word.

use crate::descriptor::{Effect, ToolDescriptor};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeSet;
use std::sync::Arc;

/// The three permission modes, ordered by what they allow:
/// `Readonly < Ask < Bypass`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionMode {
    /// Allow reads and rupu's own bookkeeping; deny workspace writes and
    /// external actions outright; cap children at readonly.
    Readonly,
    /// Prompt the operator before each write or external call. Default.
    Ask,
    /// Allow every call without prompting. Use for unattended runs.
    Bypass,
}

/// A mode word that isn't `ask`, `bypass` or `readonly`.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("unknown permission mode `{0}` (expected ask, bypass or readonly)")]
pub struct UnknownMode(pub String);

impl PermissionMode {
    /// Parse the mode word used by `--mode`, frontmatter `permissionMode:`
    /// and config `permission_mode`. The one parser.
    pub fn parse(s: &str) -> Result<Self, UnknownMode> {
        match s {
            "ask" => Ok(Self::Ask),
            "bypass" => Ok(Self::Bypass),
            "readonly" => Ok(Self::Readonly),
            other => Err(UnknownMode(other.to_string())),
        }
    }

    /// The mode word.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ask => "ask",
            Self::Bypass => "bypass",
            Self::Readonly => "readonly",
        }
    }
}

impl std::fmt::Display for PermissionMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// What the operator is asked about.
#[derive(Debug, Clone)]
pub struct PromptRequest<'a> {
    /// Canonical name of the tool.
    pub tool: &'a str,
    pub effect: Effect,
    pub input: &'a Value,
}

/// The operator's answer to a prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptAnswer {
    /// Allow this one call.
    Allow,
    /// Allow this tool (only this tool) for the rest of the run.
    AllowAlways,
    /// Deny this one call; the agent sees `permission_denied`.
    Deny,
    /// Stop the run.
    Stop,
}

/// Asks the operator about one call. Implemented by the CLI's TTY prompt.
/// One prompter may be shared by a run and its sub-agents, so an
/// implementation serializes its own prompts.
pub trait Prompter: Send + Sync {
    fn ask(&self, req: &PromptRequest<'_>) -> PromptAnswer;
}

/// Why a call was denied.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DenyReason {
    /// Readonly mode denies workspace writes and external actions.
    Readonly,
    /// The operator answered no.
    OperatorDenied,
}

/// The decision for one call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    Allow,
    /// Allowed only because ask mode has no operator in this run (D5): the
    /// caller reports that once (`permission_mode_degraded`).
    AllowDegraded,
    Deny {
        reason: DenyReason,
    },
    /// Allowed; any child run this call starts is capped at `ceiling`.
    Spawn {
        ceiling: PermissionMode,
    },
    /// The operator stopped the run.
    Stop,
}

/// The canonical names of the tools the operator chose "allow always" for in
/// this run. Keyed by tool, never by mode: allowing one tool always never
/// allows another (T4).
#[derive(Debug, Clone, Default)]
pub struct AllowAlways(BTreeSet<&'static str>);

impl AllowAlways {
    pub fn contains(&self, tool: &str) -> bool {
        self.0.contains(tool)
    }

    pub fn insert(&mut self, tool: &'static str) {
        self.0.insert(tool);
    }
}

/// A run's permission policy: its mode and, when an operator is present, the
/// prompter that asks them.
#[derive(Clone)]
pub struct PermissionPolicy {
    mode: PermissionMode,
    prompter: Option<Arc<dyn Prompter>>,
}

impl std::fmt::Debug for PermissionPolicy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PermissionPolicy")
            .field("mode", &self.mode)
            .field("prompter", &self.prompter.as_ref().map(|_| "<prompter>"))
            .finish()
    }
}

impl PermissionPolicy {
    pub fn new(mode: PermissionMode, prompter: Option<Arc<dyn Prompter>>) -> Self {
        Self { mode, prompter }
    }

    /// `mode` with no operator to prompt (detached runs, workflow steps,
    /// sessions, flows, tests).
    pub fn unattended(mode: PermissionMode) -> Self {
        Self::new(mode, None)
    }

    /// Bypass with no prompter.
    pub fn bypass() -> Self {
        Self::unattended(PermissionMode::Bypass)
    }

    pub fn mode(&self) -> PermissionMode {
        self.mode
    }

    pub fn prompter(&self) -> Option<&Arc<dyn Prompter>> {
        self.prompter.as_ref()
    }

    /// Decide one call of the tool `d` with `input`. `always` is the run's
    /// "allow always" set; an `AllowAlways` answer adds `d`'s canonical name.
    pub fn decide(&self, d: &ToolDescriptor, input: &Value, always: &mut AllowAlways) -> Decision {
        self.decide_call(d.name, d.effect, input, always)
    }

    /// [`Self::decide`] for a tool known by canonical name and effect rather
    /// than by descriptor (the MCP dispatcher's catalog entries).
    pub fn decide_call(
        &self,
        tool: &'static str,
        effect: Effect,
        input: &Value,
        always: &mut AllowAlways,
    ) -> Decision {
        match effect {
            Effect::Read | Effect::Record => Decision::Allow,
            Effect::Spawn => Decision::Spawn { ceiling: self.mode },
            Effect::Write | Effect::External => match self.mode {
                PermissionMode::Bypass => Decision::Allow,
                PermissionMode::Readonly => Decision::Deny {
                    reason: DenyReason::Readonly,
                },
                PermissionMode::Ask => {
                    let Some(prompter) = self.prompter.as_ref() else {
                        return Decision::AllowDegraded;
                    };
                    if always.contains(tool) {
                        return Decision::Allow;
                    }
                    let req = PromptRequest {
                        tool,
                        effect,
                        input,
                    };
                    match prompter.ask(&req) {
                        PromptAnswer::Allow => Decision::Allow,
                        PromptAnswer::AllowAlways => {
                            always.insert(tool);
                            Decision::Allow
                        }
                        PromptAnswer::Deny => Decision::Deny {
                            reason: DenyReason::OperatorDenied,
                        },
                        PromptAnswer::Stop => Decision::Stop,
                    }
                }
            },
        }
    }

    /// The child ceiling rule (D7): the most restrictive of this run's mode
    /// and the child's own declared `permissionMode`.
    pub fn ceiling_for_child(&self, child_declared: Option<PermissionMode>) -> PermissionMode {
        child_cap(self.mode, child_declared)
    }

    /// The policy a child run started under `ceiling` runs with: the capped
    /// mode, and this run's prompter (an interactive parent's operator
    /// answers its children's prompts too).
    pub fn for_child(
        ceiling: PermissionMode,
        child_declared: Option<PermissionMode>,
        prompter: Option<Arc<dyn Prompter>>,
    ) -> Self {
        Self::new(child_cap(ceiling, child_declared), prompter)
    }
}

/// `min(ceiling, child_declared)`; an undeclared child gets the ceiling.
fn child_cap(ceiling: PermissionMode, child_declared: Option<PermissionMode>) -> PermissionMode {
    match child_declared {
        Some(child) => ceiling.min(child),
        None => ceiling,
    }
}

/// The notice the agent loop writes, once, the first time ask mode allows a
/// write or external call because the run has no operator (D5).
pub const DEGRADED_NOTICE_KIND: &str = "permission_mode_degraded";
pub const DEGRADED_NOTICE_MESSAGE: &str =
    "ask mode has no operator in this run; write/external tools run without prompting";
