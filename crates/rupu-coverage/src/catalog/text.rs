//! Small text helpers shared across catalog rendering and tools.

/// Return the first sentence of `text` (up to the first ". " or ".\n"),
/// capped at 200 bytes on a UTF-8 char boundary, with newlines collapsed
/// to spaces. Used for one-line concern summaries.
pub fn first_sentence(text: &str) -> String {
    let trimmed = text.trim();
    let mut end_cap = trimmed.len().min(200);
    while end_cap < trimmed.len() && !trimmed.is_char_boundary(end_cap) {
        end_cap -= 1;
    }
    let mut end = end_cap;
    if let Some(idx) = trimmed[..end_cap].find(". ") {
        end = idx + 1;
    } else if let Some(idx) = trimmed[..end_cap].find(".\n") {
        end = idx + 1;
    }
    trimmed[..end].replace('\n', " ").trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_sentence_handles_trailing_period() {
        assert_eq!(first_sentence("Short summary."), "Short summary.");
        assert_eq!(first_sentence("First. Second."), "First.");
        assert_eq!(first_sentence("Multiline\nsummary."), "Multiline summary.");
    }

    #[test]
    fn first_sentence_is_utf8_safe_past_cap() {
        // 200+ bytes of multi-byte chars must not panic.
        let long = "é".repeat(150); // 300 bytes, no sentence break
        let _ = first_sentence(&long); // must not panic
    }
}
