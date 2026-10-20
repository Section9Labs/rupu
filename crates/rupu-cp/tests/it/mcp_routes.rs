//! Lockstep: every registered CP route is an operator tool (`ops()`), an
//! `EXCLUDED` route; the operator tool names
//! are pinned by `tests/fixtures/mcp/tool_names.json`, and every tool's
//! schema and description follow the listing rules.
//!
//! Regenerate the name fixture after a deliberate change with
//! `BLESS=1 cargo test -p rupu-cp --test it mcp_routes::`.

use rupu_cp::host::messages::MessageRoute;
use rupu_cp::mcp::operator_source::OperatorSource;
use rupu_cp::mcp::ops::{ops, EXCLUDED};
use rupu_mcp::ToolSource as _;
use serde_json::Value;
use std::collections::BTreeSet;
use std::path::Path;

fn tool_names_fixture() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/mcp/tool_names.json")
}

/// The pinned operator tool names, sorted.
pub(super) fn pinned_tool_names() -> Vec<String> {
    let text =
        std::fs::read_to_string(tool_names_fixture()).expect("fixture missing: run with BLESS=1");
    serde_json::from_str(&text).unwrap()
}

#[test]
fn tool_names_are_pinned() {
    let mut names: Vec<String> = ops().iter().map(|o| o.name.to_string()).collect();
    names.sort();
    if std::env::var("BLESS").is_ok() {
        let rows: Vec<String> = names.iter().map(|n| format!("  {n:?}")).collect();
        let path = tool_names_fixture();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, format!("[\n{}\n]\n", rows.join(",\n"))).unwrap();
    }
    assert_eq!(
        names,
        pinned_tool_names(),
        "ops() and tests/fixtures/mcp/tool_names.json differ"
    );
}

fn find_ref(v: &Value) -> Option<String> {
    match v {
        Value::Object(o) => o
            .get("$ref")
            .map(|r| r.to_string())
            .or_else(|| o.values().find_map(find_ref)),
        Value::Array(a) => a.iter().find_map(find_ref),
        _ => None,
    }
}

#[test]
fn every_schema_is_flat_and_described() {
    let tools = OperatorSource::new(axum::Router::new()).tools();
    assert_eq!(tools.len(), ops().len());
    for t in tools {
        let s = &t.input_schema;
        assert_eq!(s["type"], "object", "{}: {s}", t.name);
        let props = s["properties"].as_object().expect("properties");
        for r in s["required"].as_array().expect("required") {
            let r = r.as_str().unwrap();
            assert!(
                props.contains_key(r),
                "{}: required `{r}` has no property",
                t.name
            );
        }
        assert_eq!(find_ref(s), None, "{}: a `$ref` is left in {s}", t.name);
        let first = t.description.lines().next().unwrap_or("");
        assert!(
            first.chars().count() <= 120,
            "{}: first line is {} chars: {first}",
            t.name,
            first.chars().count()
        );
        assert!(
            first.ends_with('.'),
            "{}: first line ends without a period: {first}",
            t.name
        );
    }
}

const METHODS: [&str; 5] = ["get", "post", "put", "patch", "delete"];

fn rs_files(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
    for e in std::fs::read_dir(dir).unwrap() {
        let p = e.unwrap().path();
        if p.is_dir() {
            rs_files(&p, out);
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push(p);
        }
    }
}

/// Source with comments and `#[cfg(test)] mod` tails removed.
fn production_source(text: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < lines.len() {
        let l = lines[i];
        if l.trim() == "#[cfg(test)]"
            && lines
                .get(i + 1)
                .is_some_and(|n| n.trim_start().starts_with("mod ") && !n.trim_end().ends_with(';'))
        {
            // skip the module: balanced braces from its opening line
            let mut depth = 0i32;
            let mut j = i + 1;
            loop {
                for c in lines[j].chars() {
                    match c {
                        '{' => depth += 1,
                        '}' => depth -= 1,
                        _ => {}
                    }
                }
                if depth <= 0 && lines[j].contains('}') || j + 1 >= lines.len() {
                    break;
                }
                j += 1;
            }
            i = j + 1;
            continue;
        }
        if !l.trim_start().starts_with("//") {
            out.push_str(l);
            out.push('\n');
        }
        i += 1;
    }
    out
}

fn scan(src: &str, found: &mut BTreeSet<(String, String)>) {
    let b: Vec<char> = src.chars().collect();
    let pat: Vec<char> = ".route(".chars().collect();
    let mut i = 0;
    while i + pat.len() < b.len() {
        if b[i..i + pat.len()] != pat[..] {
            i += 1;
            continue;
        }
        let mut j = i + pat.len();
        while b[j].is_whitespace() {
            j += 1;
        }
        i = j;
        if b[j] != '"' {
            continue; // computed path (the generated message routes)
        }
        j += 1;
        let mut path = String::new();
        while b[j] != '"' {
            path.push(b[j]);
            j += 1;
        }
        j += 1;
        // the method-router expression, up to the route call's closing paren
        let mut depth = 0i32;
        while j < b.len() {
            match b[j] {
                '"' => {
                    j += 1;
                    while b[j] != '"' {
                        if b[j] == '\\' {
                            j += 1;
                        }
                        j += 1;
                    }
                }
                '(' | '{' | '[' => {
                    if b[j] == '(' && depth == 0 {
                        let mut k = j;
                        while k > 0 && (b[k - 1].is_alphanumeric() || b[k - 1] == '_') {
                            k -= 1;
                        }
                        let ident: String = b[k..j].iter().collect();
                        if METHODS.contains(&ident.as_str()) {
                            found.insert((ident.to_uppercase(), path.clone()));
                        }
                    }
                    depth += 1;
                }
                ')' | '}' | ']' => {
                    depth -= 1;
                    if depth < 0 {
                        break;
                    }
                }
                _ => {}
            }
            j += 1;
        }
    }
}

fn expand(path: &str) -> Vec<String> {
    if path.contains("{root}") {
        MessageRoute::ALL
            .iter()
            .map(|r| path.replace("{root}", r.segment()))
            .collect()
    } else {
        vec![path.to_string()]
    }
}

#[test]
fn every_route_is_a_tool_or_listed() {
    let mut files = Vec::new();
    rs_files(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("src"),
        &mut files,
    );
    let mut found = BTreeSet::new();
    for f in &files {
        scan(
            &production_source(&std::fs::read_to_string(f).unwrap()),
            &mut found,
        );
    }
    for r in MessageRoute::ALL {
        let base = format!("/api/{}/:id/messages", r.segment());
        found.insert(("GET".into(), base.clone()));
        found.insert(("POST".into(), base.clone()));
        found.insert(("GET".into(), format!("{base}/stream")));
    }
    println!("scanned {} method+path pairs", found.len());
    assert!(found.len() >= 155, "scanner found only {}", found.len());

    let mut tools = BTreeSet::new();
    for op in ops() {
        for p in expand(op.path) {
            tools.insert((op.method.as_str().to_string(), p));
        }
    }
    let excluded: BTreeSet<_> = EXCLUDED
        .iter()
        .map(|(m, p, _)| (m.to_string(), p.to_string()))
        .collect();

    let listed: BTreeSet<_> = tools.iter().chain(&excluded).cloned().collect();
    let missing: Vec<_> = found.difference(&listed).collect();
    for (m, p) in &missing {
        println!("    (\"{m}\", \"{p}\"),");
    }
    assert!(missing.is_empty(), "{} routes are unlisted", missing.len());

    let stale: Vec<_> = listed.difference(&found).collect();
    assert!(stale.is_empty(), "listed but not registered: {stale:?}");
}

/// The spec's exclusions (M4) plus the MCP endpoint itself, as a literal.
#[test]
fn excluded_is_exactly_the_spec_list() {
    let spec: BTreeSet<(&str, &str)> = [
        ("GET", "/api/runs/:id/log"),
        ("GET", "/api/events/stream"),
        ("GET", "/api/transcript/stream"),
        ("GET", "/api/runs/:id/messages/stream"),
        ("GET", "/api/sessions/:id/messages/stream"),
        ("GET", "/api/agentiflows/:id/messages/stream"),
        ("GET", "/api/runs/:id/coverage"),
        ("GET", "/api/runs/:id/netflow/stream"),
        ("GET", "/api/findings/artifacts/:sha256"),
        ("POST", "/api/workspace/stage"),
        ("GET", "/api/workspace/delta"),
        ("DELETE", "/api/workspace/discard"),
        ("POST", "/api/skills/bundle/missing"),
        ("PUT", "/api/skills/bundle/:hash"),
        ("GET", "/api/node/connect"),
        ("GET", "/healthz"),
        ("GET", "/api/fs/browse"),
        ("POST", "/mcp"),
    ]
    .into_iter()
    .collect();
    let have: BTreeSet<(&str, &str)> = EXCLUDED.iter().map(|(m, p, _)| (*m, *p)).collect();
    assert_eq!(have, spec);
    assert_eq!(EXCLUDED.len(), spec.len(), "EXCLUDED has a duplicate");
}

#[test]
fn the_operator_tool_count_is_pinned() {
    assert_eq!(ops().len(), 185);
    assert_eq!(pinned_tool_names().len(), 185);
}
