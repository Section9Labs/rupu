//! The children collector (spec W7 §3.6): before every turn of a run that
//! can launch, one status line per child it has not collected yet, so an
//! agent that dispatched asynchronously always knows what is still out
//! without polling `join`.

use std::sync::Arc;
use std::time::Duration;

use rupu_agent::collector::{Cadence, Injection, InjectionKind, TurnCollector, TurnContext};
use rupu_tools::{ChildState, ChildStatus, Launcher, RunIdentity};

/// Reports a run's uncollected children every turn.
pub struct ChildrenCollector {
    launcher: Arc<dyn Launcher>,
    run: Arc<RunIdentity>,
}

impl ChildrenCollector {
    pub fn new(launcher: Arc<dyn Launcher>, run: Arc<RunIdentity>) -> Self {
        Self { launcher, run }
    }
}

impl TurnCollector for ChildrenCollector {
    fn name(&self) -> &str {
        "children"
    }

    fn collect(&self, _ctx: &TurnContext) -> Vec<Injection> {
        let children = self.launcher.children(&self.run);
        if children.is_empty() {
            return Vec::new();
        }
        vec![Injection {
            source: "children".into(),
            kind: InjectionKind::Status,
            cadence: Cadence::EveryTurn,
            priority: 60,
            content: render(&children),
        }]
    }
}

/// `N children not joined:` and a line per child:
/// `ch_… running (recon, 2m)`.
pub fn render(children: &[ChildStatus]) -> String {
    let n = children.len();
    let mut out = format!(
        "{n} {} not joined (collect with `join`; unjoined children are stopped when you finish):",
        if n == 1 { "child" } else { "children" }
    );
    for c in children {
        let state = match c.state {
            ChildState::Done if c.success == Some(false) => "done (unsuccessful)",
            s => s.as_str(),
        };
        out.push_str(&format!(
            "\n- {} {} ({} {}, {})",
            c.handle,
            state,
            c.kind.as_str(),
            c.name,
            elapsed(c.elapsed)
        ));
    }
    out
}

fn elapsed(d: Duration) -> String {
    let s = d.as_secs();
    match s {
        0..=59 => format!("{s}s"),
        60..=3599 => format!("{}m", s / 60),
        _ => format!("{}h{}m", s / 3600, (s % 3600) / 60),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rupu_tools::ChildKind;

    fn child(handle: &str, state: ChildState, secs: u64) -> ChildStatus {
        ChildStatus {
            handle: handle.into(),
            kind: ChildKind::Agent,
            name: "recon".into(),
            run_id: "run_1".into(),
            codename: None,
            address: None,
            transcript: None,
            state,
            success: None,
            output: None,
            error: None,
            cause: None,
            tokens: None,
            duration_ms: None,
            elapsed: Duration::from_secs(secs),
        }
    }

    #[test]
    fn one_line_per_child_with_its_age() {
        let text = render(&[
            child("ch_1", ChildState::Running, 125),
            child("ch_2", ChildState::Done, 7),
        ]);
        assert_eq!(
            text,
            "2 children not joined (collect with `join`; unjoined children are stopped when you finish):\n\
             - ch_1 running (agent recon, 2m)\n\
             - ch_2 done (agent recon, 7s)"
        );
    }
}
