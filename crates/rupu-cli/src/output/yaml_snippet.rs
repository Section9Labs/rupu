//! Friendly "code snippet" rendering for input-validation errors.
//!
//! When `run_workflow` rejects user-provided inputs (missing required,
//! type mismatch, undeclared, enum mismatch), the typed error from
//! the orchestrator carries only the input name. The CLI is the only
//! layer that has both the workflow file path and its YAML source, so
//! it pinpoints the offending declaration here and renders a
//! Cargo-style snippet alongside the headline.
//!
//! Output shape (color-stripped):
//!
//! ```text
//! ✗ error: input `subject` is required but was not provided
//!    --> /path/to/.rupu/workflows/dispatch-demo.yaml:5
//!     |
//!  3  | inputs:
//!  4  |   subject:
//!  5  |     type: string
//!  6  |     required: true
//!     |
//!    help: pass `--input subject=<value>` on the command line
//! ```
//!
//! The headline is delegated to `RunWorkflowError`'s `Display`; this
//! module only formats the location + snippet + hint into a single
//! string that the caller wraps into `anyhow::anyhow!(...)`. Diag's
//! `fail()` then prints it verbatim.
//!
//! When the input name can't be located in the YAML (e.g. the user
//! passed an `--input` for a name that isn't declared), we fall back
//! to a no-snippet variant — still better than the bare headline.

use crate::cmd::ui::{ColorMode, UiPrefs};
use crate::output::diag::prefs_for_diag;
use crate::output::palette::{self, BRAND_300, DIM, FAILED};
use rupu_orchestrator::runner::RunWorkflowError;
use std::fmt::Write;
use std::path::Path;

/// If `err` is one of the input-validation variants, render a
/// snippet-enriched message; otherwise fall back to the typed
/// error's `Display`. The result is embedded into `anyhow::anyhow!`
/// at the call site, so no trailing newline.
pub fn render_input_error(err: &RunWorkflowError, path: &Path, body: &str) -> String {
    let (name, hint) = match err {
        RunWorkflowError::MissingRequiredInput { name } => (
            name,
            format!("pass `--input {name}=<value>` on the command line"),
        ),
        RunWorkflowError::UndeclaredInput { name } => (
            name,
            format!(
                "remove `--input {name}=...` or declare `{name}` under `inputs:` in the workflow"
            ),
        ),
        RunWorkflowError::InputNotInEnum { name, allowed, .. } => {
            (name, format!("expected one of: {}", allowed.join(", ")))
        }
        RunWorkflowError::InputTypeMismatch { name, ty, .. } => {
            (name, format!("expected a {ty} value"))
        }
        _ => return err.to_string(),
    };

    let prefs = prefs_for_diag(false);
    let headline = err.to_string();
    let snippet = build_snippet(body, name);

    let mut out = String::new();
    out.push_str(&headline);
    out.push('\n');
    let _ = write!(
        out,
        "{}",
        colored(&format!("   --> {}", path.display()), DIM, &prefs)
    );
    if let Some(snip) = snippet {
        out.push('\n');
        out.push_str(&format_snippet(&snip, &prefs));
    }
    out.push('\n');
    let _ = write!(out, "{}", colored("   help: ", BRAND_300, &prefs));
    out.push_str(&hint);
    out
}

struct SnippetBlock {
    /// 0-based file line index of the input declaration line.
    target_idx: usize,
    /// Inclusive line range to display (0-based file line indices).
    start: usize,
    end: usize,
    lines: Vec<String>,
}

/// Locate the `<name>:` declaration under `inputs:` in `body` and
/// return the lines around it. Best-effort scanner — YAML is
/// indent-sensitive but we only need to disambiguate the inputs
/// block from the (similarly-indented) steps block. Matches both
/// `inputs:` at column 0 and any indented inputs (workflows nested
/// in larger documents — currently rupu only supports top-level).
fn build_snippet(body: &str, name: &str) -> Option<SnippetBlock> {
    let lines: Vec<&str> = body.lines().collect();
    let inputs_idx = lines.iter().position(|l| l.trim_end() == "inputs:")?;
    let inputs_indent = leading_ws(lines[inputs_idx]);

    // Find `<name>:` as a child of inputs — same trimmed prefix, indent
    // strictly greater than `inputs:`'s indent.
    let target_prefix = format!("{name}:");
    let target_idx = lines
        .iter()
        .enumerate()
        .skip(inputs_idx + 1)
        .find_map(|(i, l)| {
            let lws = leading_ws(l);
            // First line at-or-below the inputs indent ends the block.
            if !l.trim().is_empty() && lws <= inputs_indent {
                return None;
            }
            let trimmed = l.trim_start();
            if trimmed == target_prefix.trim_end_matches(':') || trimmed.starts_with(&target_prefix)
            {
                Some(i)
            } else {
                None
            }
        })?;

    // Show 1 line before the target plus all of the target's children
    // (lines indented deeper than the target's own indent). End at the
    // first line at or below the target's indent.
    let target_indent = leading_ws(lines[target_idx]);
    let mut end = target_idx + 1;
    while end < lines.len() {
        let l = lines[end];
        if l.trim().is_empty() {
            end += 1;
            continue;
        }
        if leading_ws(l) > target_indent {
            end += 1;
        } else {
            break;
        }
    }
    // Trim trailing blanks from the snippet so it ends on a real line.
    while end > target_idx + 1 && lines[end - 1].trim().is_empty() {
        end -= 1;
    }
    let start = target_idx.saturating_sub(1);

    Some(SnippetBlock {
        target_idx,
        start,
        end,
        lines: lines[start..end].iter().map(|s| s.to_string()).collect(),
    })
}

fn leading_ws(s: &str) -> usize {
    s.chars().take_while(|c| *c == ' ').count()
}

fn format_snippet(s: &SnippetBlock, prefs: &UiPrefs) -> String {
    // Width sized to the largest 1-based line number in view.
    let max_lineno = s.end;
    let width = max_lineno.to_string().len().max(2);
    let mut out = String::new();
    let _ = writeln!(
        out,
        "{}",
        colored(&format!("    {:>width$} |", "", width = width), DIM, prefs)
    );
    for (offset, raw) in s.lines.iter().enumerate() {
        let file_idx = s.start + offset;
        let lineno = file_idx + 1;
        let is_target = file_idx == s.target_idx;
        let arrow = if is_target { ">" } else { " " };
        let arrow_str = colored(
            &format!(" {} ", arrow),
            if is_target { FAILED } else { DIM },
            prefs,
        );
        let lineno_str = colored(&format!("{:>width$} |", lineno, width = width), DIM, prefs);
        let _ = writeln!(out, "{arrow_str}{lineno_str} {raw}");
    }
    let _ = write!(
        out,
        "{}",
        colored(&format!("    {:>width$} |", "", width = width), DIM, prefs)
    );
    out
}

fn colored(s: &str, c: owo_colors::Rgb, prefs: &UiPrefs) -> String {
    if matches!(prefs.color, ColorMode::Never) {
        s.to_string()
    } else {
        let mut buf = String::new();
        let _ = palette::write_colored(&mut buf, s, c);
        buf
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn no_color() -> UiPrefs {
        UiPrefs {
            color: ColorMode::Never,
            theme: String::new(),
            pager: crate::cmd::ui::PagerMode::Never,
        }
    }

    const SAMPLE: &str = "name: dispatch-demo\ndescription: demo\ninputs:\n  subject:\n    type: string\n    required: true\n    description: A path to review.\nsteps:\n  - id: review\n    agent: x\n    actions: []\n    prompt: hi\n";

    #[test]
    fn locates_input_declaration() {
        let block = build_snippet(SAMPLE, "subject").expect("subject must be locatable");
        // Lines (1-based): 1 name, 2 description, 3 inputs:, 4 subject:,
        // 5 type:, 6 required:, 7 description:, 8 steps:, ...
        // target is line 4 (idx 3); end is the first line at indent <= 2,
        // which is `steps:` at line 8 (idx 7). So lines [3..7] inclusive
        // of target, exclusive of steps.
        assert_eq!(block.target_idx, 3);
        assert_eq!(block.start, 2); // 1 line before target
        assert_eq!(block.end, 7);
    }

    #[test]
    fn returns_none_for_unknown_name() {
        assert!(build_snippet(SAMPLE, "nonexistent").is_none());
    }

    #[test]
    fn render_missing_required_includes_snippet_and_hint() {
        let err = RunWorkflowError::MissingRequiredInput {
            name: "subject".into(),
        };
        let path = PathBuf::from("/tmp/dispatch-demo.yaml");
        // Use a body matching SAMPLE to ensure snippet builds.
        let _ = no_color(); // sanity: the no-color helper compiles.
        let out = render_input_error(&err, &path, SAMPLE);
        assert!(out.contains("input `subject` is required but was not provided"));
        assert!(out.contains("/tmp/dispatch-demo.yaml"));
        assert!(out.contains("subject:"));
        assert!(out.contains("required: true"));
        assert!(out.contains("--input subject=<value>"));
    }

    #[test]
    fn render_falls_back_when_input_not_in_body() {
        // UndeclaredInput: `--input nope=...` for an input the YAML
        // doesn't declare. The snippet block builder returns None;
        // we still emit the headline + path + hint.
        let err = RunWorkflowError::UndeclaredInput {
            name: "nope".into(),
        };
        let path = PathBuf::from("/tmp/x.yaml");
        let out = render_input_error(&err, &path, SAMPLE);
        assert!(out.contains("not declared"));
        assert!(out.contains("/tmp/x.yaml"));
        assert!(out.contains("--input nope=...") || out.contains("declare `nope`"));
    }

    #[test]
    fn render_passes_through_non_input_errors() {
        // Non-input errors (e.g. Io) bypass the snippet path entirely.
        let err = RunWorkflowError::Io(std::io::Error::other("disk full"));
        let path = PathBuf::from("/tmp/x.yaml");
        let out = render_input_error(&err, &path, SAMPLE);
        assert_eq!(out, "io: disk full");
    }
}
