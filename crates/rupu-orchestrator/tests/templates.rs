use rupu_orchestrator::templates::{render_step_prompt, RenderMode, StepContext};

#[test]
fn renders_inputs_prompt() {
    let ctx = StepContext::new().with_input("prompt", "find the bug");
    let out = render_step_prompt(
        "Investigate: {{ inputs.prompt }}",
        &ctx,
        RenderMode::Permissive,
    )
    .unwrap();
    assert_eq!(out, "Investigate: find the bug");
}

#[test]
fn renders_prior_step_output() {
    let ctx = StepContext::new().with_step_output("investigate", "the bug is in foo()");
    let out = render_step_prompt(
        "Based on:\n{{ steps.investigate.output }}\nPropose fix.",
        &ctx,
        RenderMode::Permissive,
    )
    .unwrap();
    assert!(out.contains("the bug is in foo()"));
}

#[test]
fn missing_variable_yields_empty_string_in_v0() {
    let ctx = StepContext::new();
    // minijinja's default behavior: undefined renders as "" — fine for v0.
    let out = render_step_prompt("Hello {{ inputs.x }}!", &ctx, RenderMode::Permissive).unwrap();
    assert_eq!(out, "Hello !");
}

#[test]
fn syntax_error_returns_render_error() {
    let ctx = StepContext::new();
    assert!(render_step_prompt("{{ unclosed", &ctx, RenderMode::Permissive).is_err());
}

// ── `fromjson` (ISSUES.md I-34) ──────────────────────────────────────
//
// An `action:` step's output is a JSON *string* end to end: the MCP
// dispatcher returns `Result<String, _>`, `StepResult.output` is a
// `String`, and it reaches minijinja verbatim. Without an inverse of
// `tojson` there is no way to index it, which makes action steps
// write-only — you can call a tool but not consume its result.
// minijinja ships no `fromjson`, and before this the crate registered
// no filters at all.

#[test]
fn fromjson_makes_an_action_steps_output_indexable() {
    let ctx = StepContext::new().with_step_output("act", r#"{"number": 7, "title": "the title"}"#);
    let out = render_step_prompt(
        "Title: {{ (steps.act.output | fromjson).title }}",
        &ctx,
        RenderMode::Permissive,
    )
    .unwrap();
    assert_eq!(out, "Title: the title");
}

#[test]
fn fromjson_yields_a_real_number_not_a_string() {
    // The value must come back typed, so arithmetic and comparisons work —
    // a string "7" would render the same here but break `> 5`.
    let ctx = StepContext::new().with_step_output("act", r#"{"number": 7}"#);
    let out = render_step_prompt(
        "{% if (steps.act.output | fromjson).number > 5 %}big{% else %}small{% endif %}",
        &ctx,
        RenderMode::Permissive,
    )
    .unwrap();
    assert_eq!(out, "big");
}

#[test]
fn fromjson_indexes_into_nested_structures_and_arrays() {
    let ctx =
        StepContext::new().with_step_output("act", r#"{"items": [{"id": "a"}, {"id": "b"}]}"#);
    let out = render_step_prompt(
        "{{ (steps.act.output | fromjson)['items'][1]['id'] }}",
        &ctx,
        RenderMode::Permissive,
    )
    .unwrap();
    assert_eq!(out, "b");
}

#[test]
fn fromjson_on_invalid_json_errors_naming_the_filter() {
    // Must not panic, and must not silently yield undefined — a silent
    // undefined would render as "" and ship an empty value downstream.
    let ctx = StepContext::new().with_step_output("act", "not json at all");
    let err = render_step_prompt(
        "{{ (steps.act.output | fromjson).x }}",
        &ctx,
        RenderMode::Permissive,
    )
    .unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("fromjson"),
        "error should name the filter, got: {msg}"
    );
    // Guard against this test passing for the wrong reason: before the filter
    // existed, minijinja's "unknown filter" error ALSO mentioned `fromjson`,
    // so the assertion above alone is satisfied by the filter not existing.
    assert!(
        !msg.contains("unknown filter"),
        "the filter must exist and reject bad JSON, not be missing: {msg}"
    );
    assert!(
        msg.contains("not valid JSON"),
        "error should explain what went wrong, got: {msg}"
    );
}

#[test]
fn fromjson_round_trips_with_tojson() {
    let ctx = StepContext::new().with_step_output("act", r#"{"a": 1}"#);
    let out = render_step_prompt(
        "{{ steps.act.output | fromjson | tojson }}",
        &ctx,
        RenderMode::Permissive,
    )
    .unwrap();
    assert_eq!(out, r#"{"a":1}"#);
}
