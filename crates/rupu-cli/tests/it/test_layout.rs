//! Keeps the workspace's integration-test layout from regressing (see
//! `main.rs`): no new top-level `tests/*.rs` binaries anywhere, nothing in
//! this binary mutating process-global state, and every test in
//! `tests/serial/` holding its `ENV_LOCK`.

use std::path::{Path, PathBuf};

/// Top-level `tests/*.rs` files that must stay their own binary.
const OWN_BINARY: &[&str] = &[
    // Lowers the process's RLIMIT_NOFILE.
    "crates/rupu-agent/tests/fd_pressure.rs",
    // Raise `credential_writes::request_termination()`'s flag: process-wide
    // and never cleared, so every runner in a shared binary would abort.
    "crates/rupu-agent/tests/terminating.rs",
    "crates/rupu-agent/tests/terminating_compact_messages.rs",
    "crates/rupu-agent/tests/terminating_compaction.rs",
    "crates/rupu-agent/tests/terminating_overflow.rs",
    "crates/rupu-orchestrator/tests/terminating.rs",
];

/// Calls that change state every thread and every spawned child sees.
const PROCESS_STATE_MUTATORS: &[&str] = &["set_var", "remove_var", "set_current_dir"];

fn manifest_dir() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

fn workspace_root() -> PathBuf {
    manifest_dir().join("../..").canonicalize().unwrap()
}

fn rs_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            rs_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

/// `src` with comments blanked out and string/char literal contents
/// replaced by spaces, so neither can fake (or hide) a call or a brace.
fn code_only(src: &str) -> String {
    let b = src.as_bytes();
    let mut out = String::with_capacity(src.len());
    let mut i = 0;
    while i < b.len() {
        let rest = &src[i..];
        if rest.starts_with("//") {
            let end = rest.find('\n').unwrap_or(rest.len());
            out.extend(std::iter::repeat_n(' ', end));
            i += end;
        } else if rest.starts_with("/*") {
            let end = rest.find("*/").map_or(rest.len(), |e| e + 2);
            out.extend(
                rest[..end]
                    .chars()
                    .map(|c| if c == '\n' { '\n' } else { ' ' }),
            );
            i += end;
        } else if let Some(hashes) = rest
            .strip_prefix('r')
            .map(|r| r.len() - r.trim_start_matches('#').len())
            .filter(|&h| rest[1 + h..].starts_with('"'))
        {
            let close = format!("\"{}", "#".repeat(hashes));
            let body = 2 + hashes;
            let end = rest[body..]
                .find(&close)
                .map_or(rest.len(), |e| body + e + close.len());
            out.push('"');
            out.extend(
                rest[1..end - 1]
                    .chars()
                    .map(|c| if c == '\n' { '\n' } else { ' ' }),
            );
            out.push('"');
            i += end;
        } else if rest.starts_with('"') {
            let mut j = 1;
            while j < b.len() - i && b[i + j] != b'"' {
                j += if b[i + j] == b'\\' { 2 } else { 1 };
            }
            out.push('"');
            out.extend(
                rest[1..j]
                    .chars()
                    .map(|c| if c == '\n' { '\n' } else { ' ' }),
            );
            out.push('"');
            i += j + 1;
        } else if let Some(len) = char_literal_len(rest) {
            out.push('\'');
            out.extend(std::iter::repeat_n(' ', len - 2));
            out.push('\'');
            i += len;
        } else {
            let c = rest.chars().next().unwrap();
            out.push(c);
            i += c.len_utf8();
        }
    }
    out
}

/// Byte length of the char literal `rest` starts with (`'x'`, `'\n'`,
/// `'\''`, `'\u{2014}'`), or `None` for anything else — a lifetime included.
fn char_literal_len(rest: &str) -> Option<usize> {
    let body = rest.strip_prefix('\'')?;
    if let Some(escaped) = body.strip_prefix('\\') {
        let first = escaped.chars().next()?;
        let close = escaped[first.len_utf8()..].find('\'')?;
        Some(2 + first.len_utf8() + close + 1)
    } else {
        let c = body.chars().next()?;
        body[c.len_utf8()..]
            .starts_with('\'')
            .then(|| 2 + c.len_utf8())
    }
}

/// `(fn name, body)` for every `#[test]` / `#[tokio::test]` fn in `code`.
fn test_fns(code: &str) -> Vec<(String, String)> {
    let mut found = Vec::new();
    let mut from = 0;
    while let Some(at) = ["#[test]", "#[tokio::test"]
        .iter()
        .filter_map(|a| code[from..].find(a).map(|p| from + p))
        .min()
    {
        let fn_at = at + code[at..].find("fn ").unwrap();
        let name: String = code[fn_at + 3..]
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_')
            .collect();
        let open = fn_at + code[fn_at..].find('{').unwrap();
        let mut depth = 0;
        let mut close = open;
        for (k, c) in code[open..].char_indices() {
            match c {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        close = open + k;
                        break;
                    }
                }
                _ => {}
            }
        }
        found.push((name, code[open..=close].to_string()));
        from = close + 1;
    }
    found
}

#[test]
fn every_crate_links_its_integration_tests_as_one_binary() {
    let root = workspace_root();
    let mut stray = Vec::new();
    for krate in std::fs::read_dir(root.join("crates")).unwrap() {
        let tests = krate.unwrap().path().join("tests");
        let Ok(entries) = std::fs::read_dir(&tests) else {
            continue;
        };
        for entry in entries {
            let path = entry.unwrap().path();
            if path.is_file() && path.extension().is_some_and(|e| e == "rs") {
                let rel = path
                    .strip_prefix(&root)
                    .unwrap()
                    .to_string_lossy()
                    .into_owned();
                if !OWN_BINARY.contains(&rel.as_str()) {
                    stray.push(rel);
                }
            }
        }
    }
    stray.sort();
    assert!(
        stray.is_empty(),
        "each top-level tests/*.rs file is a separate test binary that links the \
         crate's whole dependency graph; move these into the crate's tests/it/ \
         and add a `mod` line to tests/it/main.rs: {stray:#?}"
    );
}

#[test]
fn nothing_in_tests_it_mutates_process_state() {
    let mut files = Vec::new();
    rs_files(&manifest_dir().join("tests/it"), &mut files);
    let mut offenders = Vec::new();
    for file in files {
        let code = code_only(&std::fs::read_to_string(&file).unwrap());
        for (n, line) in code.lines().enumerate() {
            for call in PROCESS_STATE_MUTATORS {
                if line.contains(&format!("{call}(")) {
                    offenders.push(format!("{}:{}: {call}", file.display(), n + 1));
                }
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "tests/it/ shares one process — and every `rupu` child it spawns — \
         across all its tests; process env/cwd changes belong in \
         tests/serial/ under ENV_LOCK: {offenders:#?}"
    );
}

#[test]
fn every_serial_test_holds_env_lock() {
    let mut files = Vec::new();
    rs_files(&manifest_dir().join("tests/serial"), &mut files);
    let mut unlocked = Vec::new();
    let mut seen = 0;
    for file in files {
        let code = code_only(&std::fs::read_to_string(&file).unwrap());
        for (name, body) in test_fns(&code) {
            seen += 1;
            if !body.contains("ENV_LOCK") {
                unlocked.push(format!("{}: {name}", file.display()));
            }
        }
    }
    assert!(
        seen > 0,
        "found no tests under tests/serial/ — is the scan broken?"
    );
    assert!(
        unlocked.is_empty(),
        "every tests/serial/ test must hold ENV_LOCK for its whole body: \
         {unlocked:#?}"
    );
}

#[test]
fn the_scanner_sees_through_comments_and_strings() {
    let code = code_only(
        "let a = \"set_var(}\"; // set_var(\nlet b = r#\"{\"#; /* } */ let c = '}';\n\
         let d = '\"'; let e = '\\''; fn f<'a>(_: &'a str) {}\nset_var(x);",
    );
    assert_eq!(code.matches("set_var(").count(), 1, "{code}");
    assert_eq!(code.matches(['{', '}']).count(), 2, "only `fn f`'s: {code}");
    let fns = test_fns("#[test]\nfn a() { if x { y } }\n#[tokio::test(flavor = \"multi_thread\")]\nasync fn b() {}");
    let names: Vec<_> = fns.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(names, ["a", "b"]);
    assert_eq!(fns[0].1, "{ if x { y } }");
}
