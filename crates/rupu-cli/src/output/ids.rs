//! Identifier compaction and fragment resolution.
//!
//! Governing rule: never display an identifier the CLI will not accept
//! back. `compact_id` shortens for display; `resolve` accepts that
//! exact compact form, plus full identifiers, bare suffixes, and
//! prefixes.
//!
//! Both halves matter for ULIDs specifically. The leading 10 characters
//! after the type prefix encode a millisecond timestamp, so records
//! created close together share a prefix — three sessions in a real
//! listing shared `ses_01KWA`. The trailing 16 characters are random
//! and are what actually disambiguate. Keeping both ends means the
//! displayed form stays sortable by eye AND uniquely identifying.

/// Leading characters kept by `compact_id`, including the `ses_` /
/// `run_` type prefix. Four prefix characters plus eight ULID
/// characters.
const HEAD: usize = 12;

/// Trailing characters kept by `compact_id`, taken from the ULID's
/// random tail.
const TAIL: usize = 4;

/// Identifiers this long or shorter are returned unchanged — below
/// this, compaction saves nothing and only costs legibility.
const MIN_COMPACT_LEN: usize = 18;

/// Shorten an identifier for display as `<12 chars>…<4 chars>`.
///
/// Identifiers of `MIN_COMPACT_LEN` characters or fewer are returned
/// verbatim. The result always resolves back via [`resolve`].
pub fn compact_id(id: &str) -> String {
    let chars: Vec<char> = id.chars().collect();
    if chars.len() <= MIN_COMPACT_LEN {
        return id.to_string();
    }
    let head: String = chars[..HEAD].iter().collect();
    let tail: String = chars[chars.len() - TAIL..].iter().collect();
    format!("{head}…{tail}")
}

/// Outcome of matching a user-supplied fragment against known
/// identifiers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resolution {
    /// Exactly one identifier matched.
    Unique(String),
    /// Nothing matched.
    NotFound,
    /// Two or more matched. Always an error at the call site — never
    /// silently pick one. Sorted for stable output.
    Ambiguous(Vec<String>),
}

/// Match `fragment` against `candidates`.
///
/// Accepted forms, in order of precedence:
/// 1. An exact full identifier.
/// 2. The compact `head…tail` form produced by [`compact_id`].
/// 3. A bare prefix or a bare suffix.
pub fn resolve(candidates: &[String], fragment: &str) -> Resolution {
    if fragment.is_empty() {
        return Resolution::NotFound;
    }

    // An exact match always wins, even when the fragment is also a
    // prefix of some longer identifier.
    if let Some(exact) = candidates.iter().find(|c| c.as_str() == fragment) {
        return Resolution::Unique(exact.clone());
    }

    let mut matches: Vec<String> = candidates
        .iter()
        .filter(|c| matches_fragment(c, fragment))
        .cloned()
        .collect();
    matches.sort();
    matches.dedup();

    match matches.len() {
        0 => Resolution::NotFound,
        1 => Resolution::Unique(matches.remove(0)),
        _ => Resolution::Ambiguous(matches),
    }
}

/// True when `fragment` identifies `id`. Splitting on the ellipsis is
/// what lets a user paste back the exact string a table printed.
fn matches_fragment(id: &str, fragment: &str) -> bool {
    if let Some((head, tail)) = fragment.split_once('…') {
        // Guard against a fragment longer than the identifier, where
        // head and tail would overlap and both match spuriously.
        if head.chars().count() + tail.chars().count() > id.chars().count() {
            return false;
        }
        return id.starts_with(head) && id.ends_with(tail);
    }
    id.starts_with(fragment) || id.ends_with(fragment)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidates() -> Vec<String> {
        vec![
            "ses_01KWA7HTYEDX0ACG93ZW26FG3M".to_string(),
            "ses_01KWAA6X3KH6BNCPXF5Q63FJM0".to_string(),
            "ses_01KWA8RA3NE9J88X6DHSH76AC8".to_string(),
            "ses_01KS1PC047QS6NG1RG644AWXTW".to_string(),
        ]
    }

    #[test]
    fn compact_shortens_long_ids() {
        assert_eq!(
            compact_id("run_01KRJDKSBE7X4J49094149WFJS"),
            "run_01KRJDKS…WFJS"
        );
    }

    #[test]
    fn compact_leaves_short_ids_untouched() {
        assert_eq!(compact_id("run_short"), "run_short");
        // Exactly at the threshold: 18 chars, returned verbatim.
        assert_eq!(compact_id("run_0123456789ABCD"), "run_0123456789ABCD");
    }

    #[test]
    fn resolves_full_id() {
        let c = candidates();
        assert_eq!(
            resolve(&c, "ses_01KWA7HTYEDX0ACG93ZW26FG3M"),
            Resolution::Unique("ses_01KWA7HTYEDX0ACG93ZW26FG3M".to_string())
        );
    }

    #[test]
    fn resolves_the_compact_form_it_printed() {
        let c = candidates();
        let shown = compact_id(&c[0]);
        assert_eq!(shown, "ses_01KWA7HT…FG3M");
        assert_eq!(resolve(&c, &shown), Resolution::Unique(c[0].clone()));
    }

    #[test]
    fn every_compact_form_round_trips() {
        // The governing rule: never display an identifier we won't
        // accept back. Assert it for every candidate, not just one.
        let c = candidates();
        for id in &c {
            assert_eq!(
                resolve(&c, &compact_id(id)),
                Resolution::Unique(id.clone()),
                "compact form of {id} failed to resolve"
            );
        }
    }

    #[test]
    fn resolves_bare_suffix() {
        let c = candidates();
        assert_eq!(resolve(&c, "26FG3M"), Resolution::Unique(c[0].clone()));
    }

    #[test]
    fn resolves_unambiguous_prefix() {
        let c = candidates();
        assert_eq!(resolve(&c, "ses_01KS"), Resolution::Unique(c[3].clone()));
    }

    #[test]
    fn ambiguous_prefix_lists_all_matches_sorted() {
        // ULIDs are timestamp-prefixed, so records created close
        // together collide on prefix. This is the common failure.
        let c = candidates();
        match resolve(&c, "ses_01KWA") {
            Resolution::Ambiguous(matches) => {
                assert_eq!(matches.len(), 3);
                // Sorted, so error output is stable across runs.
                assert_eq!(matches[0], "ses_01KWA7HTYEDX0ACG93ZW26FG3M");
                assert_eq!(matches[1], "ses_01KWA8RA3NE9J88X6DHSH76AC8");
                assert_eq!(matches[2], "ses_01KWAA6X3KH6BNCPXF5Q63FJM0");
            }
            other => panic!("expected Ambiguous, got {other:?}"),
        }
    }

    #[test]
    fn unknown_fragment_is_not_found() {
        assert_eq!(resolve(&candidates(), "zzzzzz"), Resolution::NotFound);
    }

    #[test]
    fn empty_fragment_is_not_found_never_ambiguous() {
        // An empty fragment is a prefix of everything. Returning
        // Ambiguous(all) would be technically true and useless.
        assert_eq!(resolve(&candidates(), ""), Resolution::NotFound);
    }

    #[test]
    fn exact_match_wins_over_partial() {
        let c = vec![
            "run_0123456789".to_string(),
            "run_0123456789_extra".to_string(),
        ];
        assert_eq!(
            resolve(&c, "run_0123456789"),
            Resolution::Unique("run_0123456789".to_string())
        );
    }

    #[test]
    fn empty_candidate_set_is_not_found() {
        assert_eq!(resolve(&[], "anything"), Resolution::NotFound);
    }
}
