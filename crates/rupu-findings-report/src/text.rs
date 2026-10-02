//! Small text helpers shared by the block builder and the Markdown emitter.

/// Collapse line breaks so a value can never open a new Markdown block
/// (heading, rule, list item) from inside a title, field, step or cell.
pub(crate) fn one_line(s: &str) -> String {
    s.split(['\n', '\r'])
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// Length of the longest run of consecutive backticks in `s`.
pub(crate) fn longest_backtick_run(s: &str) -> usize {
    let mut longest = 0;
    let mut run = 0;
    for c in s.chars() {
        if c == '`' {
            run += 1;
            longest = longest.max(run);
        } else {
            run = 0;
        }
    }
    longest
}

/// A code-block language tag safe to place in an HTML class or a Typst
/// `lang:` string. `Block::Code.lang` is agent-supplied and only advisory, so
/// each emitter re-cleans it here instead of trusting the field: characters
/// outside `[A-Za-z0-9_+.#-]` are dropped, and an empty result means no tag.
pub(crate) fn safe_lang(lang: Option<&str>) -> Option<String> {
    let s: String = lang?
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '+' | '.' | '-' | '#'))
        .collect();
    (!s.is_empty()).then_some(s)
}

/// `s` with every `;` escaped as `\;`, which Markdown shows as `;`. The
/// ticket references field joins tickets with `; `, so a `;` in a ticket's
/// own text is escaped to keep each ticket whole for `import`
/// ([`split_unescaped_semicolons`], [`unescape_semicolons`]). Code spans get
/// no exception: one could pair across two tickets' unmatched backticks and
/// hide the `; ` between them, and a ticket rarely holds code (inside a span
/// the escape shows as written).
pub(crate) fn escape_semicolons(s: &str) -> String {
    s.replace(';', "\\;")
}

/// [`escape_semicolons`] undone: `\;` is `;`.
pub(crate) fn unescape_semicolons(s: &str) -> String {
    s.replace("\\;", ";")
}

/// `s` split at each `;` that is not escaped (`\;`).
pub(crate) fn split_unescaped_semicolons(s: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut from = 0;
    for (i, c) in s.char_indices() {
        if c == ';' && !s[..i].ends_with('\\') {
            out.push(&s[from..i]);
            from = i + 1;
        }
    }
    out.push(&s[from..]);
    out
}

/// `s` split at each `;` that is not in a code span (a run of backticks up
/// to the next run of exactly as many; one with no match is literal).
pub(crate) fn split_semicolons_outside_code(s: &str) -> Vec<&str> {
    let b = s.as_bytes();
    let run_at = |i: usize| b[i..].iter().take_while(|c| **c == b'`').count();
    let mut out = Vec::new();
    let (mut from, mut i) = (0, 0);
    while i < b.len() {
        match b[i] {
            b'`' => {
                let n = run_at(i);
                // The closing run: the next run of exactly `n` backticks.
                let mut j = i + n;
                let close = loop {
                    let Some(p) = b[j..].iter().position(|c| *c == b'`') else {
                        break None;
                    };
                    let m = run_at(j + p);
                    if m == n {
                        break Some(j + p + m);
                    }
                    j += p + m;
                };
                i = close.unwrap_or(i + n);
            }
            b';' => {
                out.push(&s[from..i]);
                from = i + 1;
                i += 1;
            }
            _ => i += 1,
        }
    }
    out.push(&s[from..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn semicolons_escape_and_round_trip() {
        let s = r"Fix in 3.1; backport `a; b` too \; done";
        let e = escape_semicolons(s);
        assert_eq!(e, r"Fix in 3.1\; backport `a\; b` too \\; done");
        assert_eq!(unescape_semicolons(&e), s);
        // Two tickets with an unmatched backtick each still split.
        let joined = format!(
            "{}; {}",
            escape_semicolons("A `x"),
            escape_semicolons("B `y; z")
        );
        assert_eq!(split_unescaped_semicolons(&joined), ["A `x", r" B `y\; z"]);
    }

    #[test]
    fn a_semicolon_in_a_code_span_does_not_split() {
        assert_eq!(
            split_semicolons_outside_code("NB-42 (`a;b`); ``c;`d``; e ` f; g"),
            ["NB-42 (`a;b`)", " ``c;`d``", " e ` f", " g"]
        );
    }
}
