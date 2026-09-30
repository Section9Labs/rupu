//! Per-run codename assignment (spec §4.3). The static walk is a pure
//! function of the workflow, so resume recomputes the same words; dynamic
//! state (sub-agent roles, instance counters) persists to
//! `<run_dir>/codenames.json` via `SharedNamer`.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use rupu_codename::{crew_for, Codename, CrewNamer, SharedNamer};

use crate::workflow::Workflow;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum SlotKey {
    Step(String),
    Sub { step: String, sub: String },
    Panelist { step: String, agent: String },
    Fixer(String),
}

#[derive(Debug, Clone)]
pub struct RunNaming {
    namer: SharedNamer,
    crew: Codename,
    slots: BTreeMap<SlotKey, String>,
}

fn walk(wf: &Workflow, namer: &mut CrewNamer) -> BTreeMap<SlotKey, String> {
    let mut slots = BTreeMap::new();
    for step in &wf.steps {
        if let Some(agent) = &step.agent {
            slots.insert(SlotKey::Step(step.id.clone()), namer.allocate_role(agent));
        }
        if let Some(subs) = &step.parallel {
            for s in subs {
                slots.insert(
                    SlotKey::Sub {
                        step: step.id.clone(),
                        sub: s.id.clone(),
                    },
                    namer.allocate_role(&s.agent),
                );
            }
        }
        if let Some(panel) = &step.panel {
            let mut seen = BTreeSet::new();
            for p in &panel.panelists {
                if seen.insert(p.clone()) {
                    slots.insert(
                        SlotKey::Panelist {
                            step: step.id.clone(),
                            agent: p.clone(),
                        },
                        namer.allocate_role(p),
                    );
                }
            }
            if let Some(gate) = &panel.gate {
                slots.insert(
                    SlotKey::Fixer(step.id.clone()),
                    namer.allocate_role(&gate.fix_with),
                );
            }
        }
        if let Some(ap) = &step.approval {
            for s in &ap.on_reject {
                if let Some(agent) = &s.agent {
                    slots.insert(
                        SlotKey::Sub {
                            step: step.id.clone(),
                            sub: s.id.clone(),
                        },
                        namer.allocate_role(agent),
                    );
                }
            }
        }
    }
    slots
}

impl RunNaming {
    pub fn open(wf: &Workflow, run_id: &str, run_dir: Option<&Path>) -> Self {
        let crew = crew_for(run_id);
        let mut fresh = CrewNamer::new(crew.clone());
        let slots = walk(wf, &mut fresh);
        let namer = match run_dir {
            Some(dir) => SharedNamer::open_or_init(dir.join("codenames.json"), || fresh),
            None => SharedNamer::in_memory(fresh),
        };
        Self {
            namer,
            crew: Codename::crew_only(crew),
            slots,
        }
    }

    pub fn namer(&self) -> SharedNamer {
        self.namer.clone()
    }

    pub fn crew(&self) -> &Codename {
        &self.crew
    }

    fn role(&self, key: SlotKey, agent: &str) -> String {
        match self.slots.get(&key) {
            Some(w) => w.clone(),
            None => self.namer.with(|n| n.canonical_role(agent)),
        }
    }

    pub fn step(&self, step_id: &str, agent: &str) -> Codename {
        self.crew
            .child(&self.role(SlotKey::Step(step_id.into()), agent), None)
    }

    pub fn unit(&self, step_id: &str, agent: &str, index: usize) -> Codename {
        let n = u32::try_from(index + 1).unwrap_or(u32::MAX);
        self.crew
            .child(&self.role(SlotKey::Step(step_id.into()), agent), Some(n))
    }

    pub fn sub(&self, step_id: &str, sub_id: &str, agent: &str) -> Codename {
        let key = SlotKey::Sub {
            step: step_id.into(),
            sub: sub_id.into(),
        };
        self.crew.child(&self.role(key, agent), None)
    }

    /// `occurrence` is `Some(k)` only when `agent` appears more than once in
    /// the panel (spec §3: a singleton panelist carries no number).
    pub fn panelist(&self, step_id: &str, agent: &str, occurrence: Option<u32>) -> Codename {
        let key = SlotKey::Panelist {
            step: step_id.into(),
            agent: agent.into(),
        };
        self.crew.child(&self.role(key, agent), occurrence)
    }

    pub fn fixer(&self, step_id: &str, agent: &str) -> Codename {
        self.crew
            .child(&self.role(SlotKey::Fixer(step_id.into()), agent), None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WF: &str = r#"
name: t
steps:
  - id: alpha
    agent: ag
    actions: []
    prompt: "a"
  - id: beta
    agent: ag
    actions: []
    prompt: "b"
  - id: fan
    agent: triage
    actions: []
    for_each: "{{ inputs.items }}"
    prompt: "c"
"#;
    const RUN: &str = "run_01J9ZQ3K4M5N6P7Q8R9S0T1V2W"; // crew jade-reef

    #[test]
    fn second_slot_of_same_def_gets_a_distinct_role() {
        let wf = Workflow::parse(WF).unwrap();
        let n = RunNaming::open(&wf, RUN, None);
        assert_eq!(n.crew().to_string(), "jade-reef");
        assert_eq!(n.step("alpha", "ag").to_string(), "jade-reef/hedgehog");
        assert_eq!(n.step("beta", "ag").to_string(), "jade-reef/heron");
        assert_eq!(
            n.unit("fan", "triage", 411).to_string(),
            "jade-reef/numbat#412"
        );
    }

    #[test]
    fn walk_is_deterministic_and_reloads_dynamic_state() {
        let wf = Workflow::parse(WF).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let a = RunNaming::open(&wf, RUN, Some(dir.path()));
        let parent = a.step("alpha", "ag");
        a.namer().with(|n| n.next_instance(&parent, "lynx"));
        let b = RunNaming::open(&wf, RUN, Some(dir.path()));
        assert_eq!(b.step("beta", "ag"), a.step("beta", "ag"));
        assert_eq!(b.namer().with(|n| n.next_instance(&parent, "lynx")), 2);
    }

    #[test]
    fn unknown_slot_falls_back_to_canonical_role() {
        let wf = Workflow::parse(WF).unwrap();
        let n = RunNaming::open(&wf, RUN, None);
        assert_eq!(n.step("ghost", "ag").to_string(), "jade-reef/hedgehog");
    }
}
