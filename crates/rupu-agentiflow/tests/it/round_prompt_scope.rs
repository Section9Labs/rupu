//! The lead's opening-round mission prompt names the authorized scope — its
//! mode and every root with its coordinates — so the lead knows which
//! targets it may work on. Later rounds don't repeat it.

use rupu_agentiflow::{render_round_prompt, BudgetStage, Digest, RoundContext, Scope};

fn ctx(round: u32) -> RoundContext {
    RoundContext {
        round,
        digest: Digest {
            goals: vec![],
            coverage: None,
            budget: BudgetStage::Ok,
            converge: false,
            steering: vec![],
            warnings: vec![],
        },
    }
}

fn scope() -> Scope {
    serde_yaml::from_str(
        r#"
authorized: true
mode: blackbox
roots:
  - kind: network:host
    host: 10.0.0.5
  - kind: web:site
    url: https://app.example.test
    port: 8443
"#,
    )
    .unwrap()
}

#[test]
fn opening_round_lists_the_scope_roots() {
    let p = render_round_prompt(&ctx(0), &[], "Map the attack surface.", &scope());
    assert!(p.contains("Authorized scope"), "{p}");
    assert!(p.contains("  mode: blackbox\n"), "{p}");
    assert!(p.contains("  - network:host host=10.0.0.5\n"), "{p}");
    assert!(
        p.contains("  - web:site port=8443 url=https://app.example.test\n"),
        "{p}"
    );
    // Scope sits with the mission framing, before the round banner.
    assert!(p.find("Authorized scope").unwrap() < p.find("Round 0").unwrap());
}

#[test]
fn later_rounds_do_not_repeat_the_scope() {
    let p = render_round_prompt(&ctx(1), &[], "Map the attack surface.", &scope());
    assert!(!p.contains("Authorized scope"), "{p}");
}
