//! `permission_table` (W1 §6.1): every effect × mode × prompter answer gives
//! the cell in the spec's decision table, and "allow always" is per tool.

use rupu_tools::{
    AllowAlways, Decision, DenyReason, Effect, PermissionMode, PermissionPolicy, PromptAnswer,
    PromptRequest, Prompter, ToolDescriptor,
};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};

const EFFECTS: [Effect; 5] = [
    Effect::Read,
    Effect::Record,
    Effect::Write,
    Effect::External,
    Effect::Spawn,
];

fn schema() -> Value {
    json!({"type": "object"})
}

static TOOL_A: ToolDescriptor = ToolDescriptor {
    name: "tool.a",
    aliases: &[],
    effect: Effect::Write,
    needs: &[],
    uses: &[],
    description: "a",
    input_schema: schema,
};

static TOOL_B: ToolDescriptor = ToolDescriptor {
    name: "tool.b",
    aliases: &[],
    effect: Effect::Write,
    needs: &[],
    uses: &[],
    description: "b",
    input_schema: schema,
};

/// A prompter that always gives `answer` and records what it was asked.
struct Scripted {
    answer: PromptAnswer,
    asked: Mutex<Vec<String>>,
}

impl Scripted {
    fn new(answer: PromptAnswer) -> Arc<Self> {
        Arc::new(Self {
            answer,
            asked: Mutex::new(Vec::new()),
        })
    }

    fn asked(&self) -> Vec<String> {
        self.asked.lock().unwrap().clone()
    }
}

impl Prompter for Scripted {
    fn ask(&self, req: &PromptRequest<'_>) -> PromptAnswer {
        self.asked.lock().unwrap().push(req.tool.to_string());
        self.answer
    }
}

fn decide(policy: &PermissionPolicy, effect: Effect) -> Decision {
    policy.decide_call("tool.x", effect, &json!({}), &mut AllowAlways::default())
}

/// The spec's table, cell by cell. `answer` is the prompter's answer, `None`
/// for a run without one.
fn expected(effect: Effect, mode: PermissionMode, answer: Option<PromptAnswer>) -> Decision {
    use PermissionMode::*;
    match (effect, mode) {
        (Effect::Read | Effect::Record, _) => Decision::Allow,
        (Effect::Spawn, m) => Decision::Spawn { ceiling: m },
        (Effect::Write | Effect::External, Bypass) => Decision::Allow,
        (Effect::Write | Effect::External, Readonly) => Decision::Deny {
            reason: DenyReason::Readonly,
        },
        (Effect::Write | Effect::External, Ask) => match answer {
            None => Decision::AllowDegraded,
            Some(PromptAnswer::Allow | PromptAnswer::AllowAlways) => Decision::Allow,
            Some(PromptAnswer::Deny) => Decision::Deny {
                reason: DenyReason::OperatorDenied,
            },
            Some(PromptAnswer::Stop) => Decision::Stop,
        },
    }
}

#[test]
fn every_effect_mode_and_answer_gives_its_table_cell() {
    let answers = [
        None,
        Some(PromptAnswer::Allow),
        Some(PromptAnswer::AllowAlways),
        Some(PromptAnswer::Deny),
        Some(PromptAnswer::Stop),
    ];
    for effect in EFFECTS {
        for mode in [
            PermissionMode::Readonly,
            PermissionMode::Ask,
            PermissionMode::Bypass,
        ] {
            for answer in answers {
                let prompter = answer.map(Scripted::new);
                let policy =
                    PermissionPolicy::new(mode, prompter.clone().map(|p| p as Arc<dyn Prompter>));
                assert_eq!(
                    decide(&policy, effect),
                    expected(effect, mode, answer),
                    "{effect:?} × {mode:?} × {answer:?}"
                );
                // The operator is asked exactly for a write/external call
                // under ask, and never otherwise.
                if let Some(p) = prompter {
                    let should_ask = mode == PermissionMode::Ask
                        && matches!(effect, Effect::Write | Effect::External);
                    assert_eq!(
                        p.asked().len(),
                        usize::from(should_ask),
                        "{effect:?} × {mode:?}"
                    );
                }
            }
        }
    }
}

#[test]
fn allow_always_covers_one_tool_not_the_run() {
    // T4: "allow always" for tool A must not allow tool B.
    let p = Scripted::new(PromptAnswer::AllowAlways);
    let policy = PermissionPolicy::new(PermissionMode::Ask, Some(p.clone()));
    let mut always = AllowAlways::default();

    assert_eq!(
        policy.decide(&TOOL_A, &json!({}), &mut always),
        Decision::Allow
    );
    assert!(always.contains("tool.a"));
    // A again: no second prompt.
    assert_eq!(
        policy.decide(&TOOL_A, &json!({}), &mut always),
        Decision::Allow
    );
    assert_eq!(p.asked(), ["tool.a"]);
    // B prompts on its own.
    policy.decide(&TOOL_B, &json!({}), &mut always);
    assert_eq!(p.asked(), ["tool.a", "tool.b"]);
    assert_eq!(policy.mode(), PermissionMode::Ask, "the mode never flips");
}

#[test]
fn a_child_never_has_more_than_its_parent() {
    use PermissionMode::*;
    let modes = [Readonly, Ask, Bypass];
    for parent in modes {
        let policy = PermissionPolicy::unattended(parent);
        assert_eq!(policy.ceiling_for_child(None), parent);
        for child in modes {
            let got = policy.ceiling_for_child(Some(child));
            assert_eq!(got, parent.min(child), "{parent:?} parent, {child:?} child");
            assert!(got <= parent);
        }
    }
    // The readonly cell of the spawn row: whatever the child declares.
    assert_eq!(
        PermissionPolicy::for_child(Readonly, Some(Bypass), None).mode(),
        Readonly
    );
}

#[test]
fn the_mode_word_has_one_parser() {
    assert_eq!(PermissionMode::parse("ask"), Ok(PermissionMode::Ask));
    assert_eq!(PermissionMode::parse("bypass"), Ok(PermissionMode::Bypass));
    assert_eq!(
        PermissionMode::parse("readonly"),
        Ok(PermissionMode::Readonly)
    );
    assert!(PermissionMode::parse("Readonly").is_err());
    assert!(PermissionMode::parse("").is_err());
    for m in [
        PermissionMode::Readonly,
        PermissionMode::Ask,
        PermissionMode::Bypass,
    ] {
        assert_eq!(PermissionMode::parse(m.as_str()), Ok(m));
    }
}
