//! A skill's name, validated per the agentskills.io rules.

use serde::Serialize;
use std::fmt;
use std::str::FromStr;

pub const MAX_NAME_LEN: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct SkillName(String);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum NameError {
    #[error("skill name is empty")]
    Empty,
    #[error("skill name is {0} characters; the limit is 64")]
    TooLong(usize),
    #[error("skill name may only contain a-z, 0-9 and '-'; found {0:?}")]
    BadChar(char),
    #[error("skill name must not start or end with '-'")]
    EdgeHyphen,
    #[error("skill name must not contain '--'")]
    DoubleHyphen,
}

impl SkillName {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl FromStr for SkillName {
    type Err = NameError;

    fn from_str(s: &str) -> Result<Self, NameError> {
        if s.is_empty() {
            return Err(NameError::Empty);
        }
        let len = s.chars().count();
        if len > MAX_NAME_LEN {
            return Err(NameError::TooLong(len));
        }
        if let Some(c) = s
            .chars()
            .find(|c| !matches!(c, 'a'..='z' | '0'..='9' | '-'))
        {
            return Err(NameError::BadChar(c));
        }
        if s.starts_with('-') || s.ends_with('-') {
            return Err(NameError::EdgeHyphen);
        }
        if s.contains("--") {
            return Err(NameError::DoubleHyphen);
        }
        Ok(Self(s.to_owned()))
    }
}

impl fmt::Display for SkillName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}
