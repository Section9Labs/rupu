//! What a run may write into its message space ([`MessagingGrant`], spec
//! F1d §3): one resolution per run, decided by the assembler, from which
//! both "is the bus offered to the tools" and the grant that offers them
//! follow, so the two can't disagree.

use rupu_tools::{AmbientGrant, GrantReason, MessagingSwitch, Service, ToolCatalog};

use super::origin::Origin;
use super::spec::LaunchSpec;

/// What a run may write into its message space (spec F1d §3). Off wins over
/// everything, an origin grant and a handed bus included (D5).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MessagingGrant {
    /// No messaging tools: the bus stays receive-only. The operator and the
    /// run's children still reach it.
    Off(OffReason),
    /// These tools, canonical, offered under this reason.
    Tools(Vec<&'static str>, GrantReason),
}

/// Who turned a run's messaging off (spec F1d D5), the nearest first.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OffReason {
    /// The agent's `messaging: false`.
    Agent,
    /// The workflow's `messaging: false`.
    Workflow,
    /// The agentiflow's `messaging: false`.
    Agentiflow,
    /// The parent's cap: a child never has more than its parent (D6).
    Parent,
    /// The launch's `--no-messaging` ([`super::Overrides::messaging`]): a
    /// placed unit's coordinator passing its workflow's switch or its own
    /// launch cap.
    Launch,
}

impl MessagingGrant {
    /// Resolve `spec`'s messaging. Off when its agent, its workflow, its
    /// agentiflow, its parent or its launch (`--no-messaging`) says off.
    /// Else its launch's tools (`--messaging-tool`), its workflow's, a flow
    /// origin's (the messaging tools of `origin_grant`, which the grant
    /// already carries) or the default (D1).
    pub fn resolve(spec: &LaunchSpec, origin_grant: Option<&AmbientGrant>) -> Self {
        if spec.agent.messaging_off() {
            return Self::Off(OffReason::Agent);
        }
        let parent_off = match (&spec.origin, &spec.link) {
            (Origin::SubAgent { parent }, _) => parent.identity.launch.messaging_off,
            (_, Some(link)) => link.no_messaging,
            _ => false,
        };
        match (&spec.origin, &spec.overrides.messaging) {
            (
                Origin::WorkflowStep {
                    messaging: MessagingSwitch::Off,
                    ..
                },
                _,
            ) => Self::Off(OffReason::Workflow),
            (
                Origin::FlowLead {
                    messaging_off: true,
                    ..
                },
                _,
            ) => Self::Off(OffReason::Agentiflow),
            _ if parent_off => Self::Off(OffReason::Parent),
            (_, MessagingSwitch::Off) => Self::Off(OffReason::Launch),
            (_, MessagingSwitch::Tools(tools))
            | (
                Origin::WorkflowStep {
                    messaging: MessagingSwitch::Tools(tools),
                    ..
                },
                _,
            ) => Self::Tools(tools.clone(), GrantReason::Ambient("workflow_messaging")),
            (Origin::FlowLead { .. } | Origin::FlowUnit { .. }, _) => match origin_grant {
                Some(g) => {
                    let catalog = ToolCatalog::builtin();
                    let tools = g
                        .tools
                        .iter()
                        .copied()
                        .filter(|n| {
                            catalog
                                .descriptor(n)
                                .is_some_and(|d| d.needs.contains(&Service::MessageBus))
                        })
                        .collect();
                    Self::Tools(tools, g.reason.clone())
                }
                None => Self::default_tools(),
            },
            _ => Self::default_tools(),
        }
    }

    fn default_tools() -> Self {
        let g = AmbientGrant::messaging_default();
        Self::Tools(g.tools, g.reason)
    }

    /// The run is offered its bus (`Service::MessageBus`).
    pub fn takes_part(&self) -> bool {
        matches!(self, Self::Tools(..))
    }

    /// The grant that offers the tools; `None` when off.
    pub fn ambient(&self) -> Option<AmbientGrant> {
        match self {
            Self::Off(_) => None,
            Self::Tools(tools, reason) => Some(AmbientGrant {
                tools: tools.clone(),
                reason: reason.clone(),
            }),
        }
    }
}
