//! Explicit continuation list overrides. Missing inherits, empty clears only
//! launch additions; agent declarations and shipped bundles remain independent.
use serde::{Deserialize, Deserializer, Serialize};

pub const FEATURE: &str = "agent.continue_options.v1";
pub const MAX_BYTES: usize = 256 * 1024;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContinuationListOverrides {
    #[serde(
        default,
        deserialize_with = "present_list",
        skip_serializing_if = "Option::is_none"
    )]
    pub skills: Option<Vec<String>>,
    #[serde(
        default,
        deserialize_with = "present_list",
        skip_serializing_if = "Option::is_none"
    )]
    pub preload_skills: Option<Vec<String>>,
    #[serde(
        default,
        deserialize_with = "present_list",
        skip_serializing_if = "Option::is_none"
    )]
    pub engagement_profiles: Option<Vec<String>>,
}
fn present_list<'de, D: Deserializer<'de>>(decoder: D) -> Result<Option<Vec<String>>, D::Error> {
    Vec::<String>::deserialize(decoder).map(Some)
}
impl ContinuationListOverrides {
    pub fn decode(raw: &str) -> Result<Self, String> {
        if raw.len() > MAX_BYTES {
            return Err("continuation lists exceed 256 KiB".into());
        }
        let value: Self =
            serde_json::from_str(raw).map_err(|e| format!("continuation lists: {e}"))?;
        value.validate()?;
        Ok(value)
    }
    pub fn validate(&self) -> Result<(), String> {
        if self.skills.is_none()
            && self.preload_skills.is_none()
            && self.engagement_profiles.is_none()
        {
            return Err("lists must contain at least one explicit selection".into());
        }
        for (name, list) in [
            ("skills", &self.skills),
            ("preload_skills", &self.preload_skills),
            ("engagement_profiles", &self.engagement_profiles),
        ] {
            if let Some(list) = list {
                if list.len() > 256
                    || list.iter().any(|v| {
                        v.len() > 1024 || v.trim().is_empty() || v.chars().any(char::is_control)
                    })
                {
                    return Err(format!("{name}: use at most 256 nonblank entries of at most 1024 bytes without control characters"));
                }
            }
        }
        if serde_json::to_vec(self).map_err(|e| e.to_string())?.len() > MAX_BYTES {
            return Err("continuation lists exceed 256 KiB".into());
        }
        for list in [&self.skills, &self.preload_skills].into_iter().flatten() {
            rupu_skills::parse_grants(list).map_err(|e| format!("continuation skills: {e}"))?;
        }
        Ok(())
    }
    pub fn resolved(
        skills: Vec<String>,
        preload_skills: Vec<String>,
        engagement_profiles: Vec<String>,
    ) -> Self {
        Self {
            skills: Some(skills),
            preload_skills: Some(preload_skills),
            engagement_profiles: Some(engagement_profiles),
        }
    }
    pub fn apply(
        &self,
        skills: &mut Vec<String>,
        preload_skills: &mut Vec<String>,
        engagement_profiles: &mut Vec<String>,
    ) {
        if let Some(value) = &self.skills {
            *skills = value.clone();
        }
        if let Some(value) = &self.preload_skills {
            *preload_skills = value.clone();
        }
        if let Some(value) = &self.engagement_profiles {
            *engagement_profiles = value.clone();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn omission_empty_and_replacement_are_independent() {
        let v =
            ContinuationListOverrides::decode(r#"{"skills":[],"engagement_profiles":["network"]}"#)
                .unwrap();
        let (mut skills, mut preload, mut profiles) =
            (vec!["old".into()], vec!["keep".into()], vec!["code".into()]);
        v.apply(&mut skills, &mut preload, &mut profiles);
        assert!(skills.is_empty());
        assert_eq!(preload, ["keep"]);
        assert_eq!(profiles, ["network"]);
        assert_eq!(
            serde_json::to_value(v).unwrap(),
            serde_json::json!({"skills":[],"engagement_profiles":["network"]})
        );
    }
    #[test]
    fn strict_shape_and_bounded_entries() {
        for raw in [
            "{}",
            r#"{"skills":null}"#,
            r#"{"skills":[],"unknown":[]}"#,
            r#"{"preload_skills":[null]}"#,
            r#"{"engagement_profiles":[""]}"#,
        ] {
            assert!(ContinuationListOverrides::decode(raw).is_err(), "{raw}");
        }
        assert!(ContinuationListOverrides {
            skills: Some(vec!["s".into(); 257]),
            ..Default::default()
        }
        .validate()
        .is_err());
        assert!(ContinuationListOverrides::decode(&" ".repeat(MAX_BYTES + 1)).is_err());
    }
    #[test]
    fn resolved_empty_selection_survives_serialization() {
        let v = ContinuationListOverrides::resolved(vec![], vec!["project:notes".into()], vec![]);
        assert_eq!(
            ContinuationListOverrides::decode(&serde_json::to_string(&v).unwrap()).unwrap(),
            v
        );
    }
}
