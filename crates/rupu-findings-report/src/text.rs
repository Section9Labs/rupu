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
