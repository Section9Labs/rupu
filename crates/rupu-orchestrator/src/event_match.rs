//! Glob-style matcher for `trigger.event:` patterns.
//!
//! Pattern syntax: literal characters match themselves; `*` matches
//! any (possibly empty) sequence of characters. No other metacharacters
//! — keep the surface tiny so workflow authors can predict behavior.
//!
//! Examples:
//! - `github.issue.*`        matches `github.issue.opened`, `github.issue.closed`, …
//! - `*.pr.merged`           matches `github.pr.merged`, `gitlab.mr.merged`
//! - `github.*.opened`       matches `github.issue.opened`, `github.pr.opened`
//! - `github.issue.opened`   matches only that exact id (no `*` = literal)
//!
//! The `*` does NOT special-case `.` boundaries — `github.*` matches
//! `github.issue.opened` (anything after `github.`). Workflow authors
//! who want vendor-segment matching just write `github.*` rather than
//! reaching for a more elaborate syntax.

/// Returns `true` when `pattern` matches `s` under glob-style rules
/// (literal + `*`).
pub fn event_matches(pattern: &str, s: &str) -> bool {
    glob_match(pattern.as_bytes(), s.as_bytes())
}

/// Iterative two-pointer matcher with backtracking on `*`. O(N*M)
/// worst case (N = pattern length, M = string length); for the bounded
/// inputs we see (~30-char event ids, patterns with 0-2 stars) this is
/// trivially fast.
fn glob_match(pat: &[u8], s: &[u8]) -> bool {
    let mut p = 0usize;
    let mut i = 0usize;
    let mut star_p: Option<usize> = None;
    let mut star_i: usize = 0;

    while i < s.len() {
        if p < pat.len() && pat[p] == b'*' {
            star_p = Some(p);
            star_i = i;
            p += 1;
        } else if p < pat.len() && pat[p] == s[i] {
            p += 1;
            i += 1;
        } else if let Some(sp) = star_p {
            // Backtrack: consume one more char into the latest `*`.
            p = sp + 1;
            star_i += 1;
            i = star_i;
        } else {
            return false;
        }
    }
    // Trailing `*`s after the input ends are still acceptable.
    while p < pat.len() && pat[p] == b'*' {
        p += 1;
    }
    p == pat.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_match() {
        assert!(event_matches("github.issue.opened", "github.issue.opened"));
        assert!(!event_matches("github.issue.opened", "github.issue.closed"));
    }

    #[test]
    fn trailing_star_matches_any_suffix() {
        assert!(event_matches("github.issue.*", "github.issue.opened"));
        assert!(event_matches("github.issue.*", "github.issue.closed"));
        assert!(event_matches("github.issue.*", "github.issue.commented"));
        assert!(!event_matches("github.issue.*", "github.pr.opened"));
    }

    #[test]
    fn leading_star_matches_any_prefix() {
        assert!(event_matches("*.pr.merged", "github.pr.merged"));
        assert!(!event_matches("*.pr.merged", "gitlab.mr.merged")); // mr != pr
        assert!(event_matches("*.mr.merged", "gitlab.mr.merged"));
    }

    #[test]
    fn middle_star_matches_any_segment() {
        assert!(event_matches("github.*.opened", "github.issue.opened"));
        assert!(event_matches("github.*.opened", "github.pr.opened"));
        assert!(!event_matches("github.*.opened", "github.issue.closed"));
    }

    #[test]
    fn star_alone_matches_anything() {
        assert!(event_matches("*", "github.issue.opened"));
        assert!(event_matches("*", ""));
    }

    #[test]
    fn star_can_match_empty() {
        assert!(event_matches("github.*opened", "github.opened"));
        assert!(event_matches("github.*opened", "github.issue.opened"));
    }

    #[test]
    fn multiple_stars() {
        assert!(event_matches("*.*.opened", "github.issue.opened"));
        assert!(event_matches("*.*.opened", "gitlab.mr.opened"));
    }

    #[test]
    fn empty_pattern_only_matches_empty_string() {
        assert!(event_matches("", ""));
        assert!(!event_matches("", "github.issue.opened"));
    }

    #[test]
    fn no_match_when_literal_diverges() {
        assert!(!event_matches("github.issue.opened", "github.issue.opened.extra"));
        assert!(!event_matches("github.pr.*", "github.issue.opened"));
    }
}
