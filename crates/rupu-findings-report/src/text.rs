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

/// Byte ranges of the inline code spans in `s`: a run of backticks up to
/// the next run of exactly as many (CommonMark). A run with no match is
/// literal backticks.
pub(crate) fn code_span_ranges(s: &str) -> Vec<std::ops::Range<usize>> {
    let b = s.as_bytes();
    let run_at = |i: usize| b[i..].iter().take_while(|c| **c == b'`').count();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if b[i] != b'`' {
            i += 1;
            continue;
        }
        let n = run_at(i);
        let mut j = i + n;
        let close = loop {
            match b[j..].iter().position(|c| *c == b'`') {
                None => break None,
                Some(p) => {
                    let at = j + p;
                    let m = run_at(at);
                    if m == n {
                        break Some(at + m);
                    }
                    j = at + m;
                }
            }
        };
        match close {
            Some(end) => {
                out.push(i..end);
                i = end;
            }
            None => i += n,
        }
    }
    out
}

/// `s` with every `;` outside a code span escaped as `\;`, which Markdown
/// shows as `;`. The ticket references field joins tickets with `; `, so a
/// `;` in a ticket's own text is escaped to keep each ticket whole for
/// `import` ([`split_unescaped_semicolons`], [`unescape_semicolons`]).
pub(crate) fn escape_semicolons(s: &str) -> String {
    let spans = code_span_ranges(s);
    let mut out = String::with_capacity(s.len());
    for (i, c) in s.char_indices() {
        if c == ';' && !spans.iter().any(|r| r.contains(&i)) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// [`escape_semicolons`] undone: `\;` outside a code span is `;`.
pub(crate) fn unescape_semicolons(s: &str) -> String {
    let spans = code_span_ranges(s);
    let mut out = String::with_capacity(s.len());
    let mut chars = s.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        let escape = c == '\\'
            && chars.peek().is_some_and(|(_, n)| *n == ';')
            && !spans.iter().any(|r| r.contains(&i));
        if !escape {
            out.push(c);
        }
    }
    out
}

/// `s` split at each `;` that is neither escaped (`\;`) nor in a code span.
pub(crate) fn split_unescaped_semicolons(s: &str) -> Vec<&str> {
    let spans = code_span_ranges(s);
    let mut out = Vec::new();
    let mut from = 0;
    for (i, c) in s.char_indices() {
        let escaped = s[..i].ends_with('\\');
        if c == ';' && !escaped && !spans.iter().any(|r| r.contains(&i)) {
            out.push(&s[from..i]);
            from = i + 1;
        }
    }
    out.push(&s[from..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn semicolons_escape_outside_code_spans_and_round_trip() {
        let s = r"Fix in 3.1; backport `a; b` and ``c;`d`` too \; done";
        let e = escape_semicolons(s);
        assert_eq!(e, r"Fix in 3.1\; backport `a; b` and ``c;`d`` too \\; done");
        assert_eq!(unescape_semicolons(&e), s);
        let joined = format!("{e}; Other NB-43");
        assert_eq!(
            split_unescaped_semicolons(&joined),
            [e.as_str(), " Other NB-43"]
        );
        // An unmatched backtick is literal: the `;` after it is escaped.
        assert_eq!(escape_semicolons("a ` b; c"), r"a ` b\; c");
    }
}
