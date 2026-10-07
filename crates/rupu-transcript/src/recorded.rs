//! A customer as a persisted record wrote it at launch.
//!
//! Three states, kept apart on disk (spec rule: a project with a customer is
//! never silently counted as none, and reassigning a project never rewrites
//! history):
//!
//! - key **absent** — a legacy record, written before customers existed. A
//!   reader may derive its customer from the project's CURRENT assignment.
//! - explicit **`null`** — the launch recorded "no customer". It stays none
//!   even if the project is assigned later.
//! - a **string** — the slug the launch ran under.
//!
//! In memory the field is a double option ([`RecordedField`]): `None` =
//! legacy, `Some(None)` = recorded none, `Some(Some(slug))` = recorded slug.
//! Declare it as
//!
//! ```ignore
//! #[serde(
//!     default,
//!     deserialize_with = "rupu_transcript::recorded::deserialize",
//!     skip_serializing_if = "Option::is_none"
//! )]
//! pub customer: rupu_transcript::recorded::RecordedField,
//! ```
//!
//! `default` maps an absent key to legacy, [`deserialize`] maps `null` to
//! `Some(None)` (plain serde would collapse it into the absent case), and
//! `skip_serializing_if` writes nothing back for a legacy record — so a
//! legacy record that is re-saved (a status update, a resume) stays legacy —
//! while a recorded none serializes as `null`. A writer always records:
//! `customer: Some(resolved_customer)`.

use serde::{Deserialize, Deserializer};

/// The on-record shape. See the module doc.
pub type RecordedField = Option<Option<String>>;

/// A borrowed view of a [`RecordedField`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Recorded<'a> {
    /// The key is absent: the record predates customers.
    Legacy,
    /// The launch recorded "no customer".
    None,
    /// The launch recorded this customer.
    Slug(&'a str),
}

impl<'a> Recorded<'a> {
    pub fn of(field: &'a RecordedField) -> Self {
        match field {
            None => Self::Legacy,
            Some(None) => Self::None,
            Some(Some(slug)) => Self::Slug(slug.as_str()),
        }
    }

    /// The recorded slug; `None` for a recorded none AND for a legacy record.
    pub fn slug(self) -> Option<&'a str> {
        match self {
            Self::Slug(s) => Some(s),
            Self::Legacy | Self::None => None,
        }
    }

    pub fn is_legacy(self) -> bool {
        matches!(self, Self::Legacy)
    }

    /// Back to the owned on-record shape.
    pub fn to_field(self) -> RecordedField {
        match self {
            Self::Legacy => None,
            Self::None => Some(None),
            Self::Slug(s) => Some(Some(s.to_string())),
        }
    }
}

/// `deserialize_with` for a [`RecordedField`]: a present key (string or
/// `null`) is `Some(..)`; an absent key never reaches this function, so the
/// field's `#[serde(default)]` makes it `None` (legacy).
pub fn deserialize<'de, D>(d: D) -> Result<RecordedField, D::Error>
where
    D: Deserializer<'de>,
{
    Option::<String>::deserialize(d).map(Some)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Serialize;

    #[derive(Debug, PartialEq, Serialize, Deserialize)]
    struct Rec {
        #[serde(
            default,
            deserialize_with = "deserialize",
            skip_serializing_if = "Option::is_none"
        )]
        customer: RecordedField,
    }

    #[test]
    fn absent_null_and_slug_stay_distinct_through_a_round_trip() {
        for (json, field, view) in [
            ("{}", None, Recorded::Legacy),
            (r#"{"customer":null}"#, Some(None), Recorded::None),
            (
                r#"{"customer":"acme"}"#,
                Some(Some("acme".to_string())),
                Recorded::Slug("acme"),
            ),
        ] {
            let rec: Rec = serde_json::from_str(json).unwrap();
            assert_eq!(rec.customer, field, "{json}");
            assert_eq!(Recorded::of(&rec.customer), view, "{json}");
            assert_eq!(serde_json::to_string(&rec).unwrap(), json);
            assert_eq!(view.to_field(), field);
        }
        assert_eq!(Recorded::Slug("acme").slug(), Some("acme"));
        assert_eq!(Recorded::None.slug(), None);
        assert!(Recorded::Legacy.is_legacy() && !Recorded::None.is_legacy());
    }
}
