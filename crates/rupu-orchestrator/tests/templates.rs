use rupu_orchestrator::templates::{render_step_prompt, StepContext};

#[test]
fn renders_inputs_prompt() {
    let ctx = StepContext::new().with_input("prompt", "find the bug");
    let out = render_step_prompt("Investigate: {{ inputs.prompt }}", &ctx).unwrap();
    assert_eq!(out, "Investigate: find the bug");
}

#[test]
fn renders_prior_step_output() {
    let ctx = StepContext::new().with_step_output("investigate", "the bug is in foo()");
    let out = render_step_prompt(
        "Based on:\n{{ steps.investigate.output }}\nPropose fix.",
        &ctx,
    )
    .unwrap();
    assert!(out.contains("the bug is in foo()"));
}

#[test]
fn missing_variable_yields_empty_string_in_v0() {
    let ctx = StepContext::new();
    // minijinja's default behavior: undefined renders as "" — fine for v0.
    let out = render_step_prompt("Hello {{ inputs.x }}!", &ctx).unwrap();
    assert_eq!(out, "Hello !");
}

#[test]
fn syntax_error_returns_render_error() {
    let ctx = StepContext::new();
    assert!(render_step_prompt("{{ unclosed", &ctx).is_err());
}
