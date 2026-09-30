use std::fmt;
use std::str::FromStr;

/// One agent in the path: `role[#n][.attempt]`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Segment {
    pub role: String,
    /// Instance number, 1-based. `None` for a statically-known singleton.
    pub n: Option<u32>,
    /// Retry attempt, ≥ 2. `None` on the first attempt.
    pub attempt: Option<u32>,
}

/// `crew[/role[#n][.a](>role[#n][.a])*]` — see the codenames spec §3.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Codename {
    pub crew: String,
    pub segments: Vec<Segment>,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
#[error("not a codename: {0:?}")]
pub struct ParseCodenameError(pub String);

impl Codename {
    pub fn crew_only(crew: impl Into<String>) -> Self {
        Self {
            crew: crew.into(),
            segments: Vec::new(),
        }
    }

    /// A child agent: the first call adds the member (`crew/role`), later
    /// calls add a sub-agent (`…>role`).
    pub fn child(&self, role: &str, n: Option<u32>) -> Self {
        let mut c = self.clone();
        c.segments.push(Segment {
            role: role.to_string(),
            n,
            attempt: None,
        });
        c
    }

    /// Mark the last segment as retry attempt `attempt` (≥ 2; 1 is a no-op).
    pub fn with_attempt(mut self, attempt: u32) -> Self {
        if attempt >= 2 {
            if let Some(last) = self.segments.last_mut() {
                last.attempt = Some(attempt);
            }
        }
        self
    }

    /// Everything after the crew (`heron#412>lynx#3`), or the crew itself when
    /// there are no segments.
    pub fn leaf(&self) -> String {
        if self.segments.is_empty() {
            return self.crew.clone();
        }
        self.segments
            .iter()
            .map(Segment::to_string)
            .collect::<Vec<_>>()
            .join(">")
    }
}

impl fmt::Display for Segment {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.role)?;
        if let Some(n) = self.n {
            write!(f, "#{n}")?;
        }
        if let Some(a) = self.attempt {
            write!(f, ".{a}")?;
        }
        Ok(())
    }
}

impl fmt::Display for Codename {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.crew)?;
        if !self.segments.is_empty() {
            write!(f, "/{}", self.leaf())?;
        }
        Ok(())
    }
}

fn is_word(s: &str) -> bool {
    let mut chars = s.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_lowercase())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
}

fn positive(s: &str) -> Option<u32> {
    s.parse::<u32>().ok().filter(|n| *n >= 1)
}

fn parse_segment(s: &str) -> Option<Segment> {
    let (head, attempt) = match s.split_once('.') {
        Some((h, a)) => (h, Some(positive(a).filter(|a| *a >= 2)?)),
        None => (s, None),
    };
    let (role, n) = match head.split_once('#') {
        Some((r, n)) => (r, Some(positive(n)?)),
        None => (head, None),
    };
    is_word(role).then(|| Segment {
        role: role.to_string(),
        n,
        attempt,
    })
}

impl FromStr for Codename {
    type Err = ParseCodenameError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let err = || ParseCodenameError(s.to_string());
        let (crew, rest) = match s.split_once('/') {
            Some((c, r)) => (c, Some(r)),
            None => (s, None),
        };
        let (color, noun) = crew.split_once('-').ok_or_else(err)?;
        if !is_word(color) || !is_word(noun) {
            return Err(err());
        }
        let segments = match rest {
            None => Vec::new(),
            Some(r) => r
                .split('>')
                .map(parse_segment)
                .collect::<Option<Vec<_>>>()
                .ok_or_else(err)?,
        };
        if rest.is_some() && segments.is_empty() {
            return Err(err());
        }
        Ok(Self {
            crew: crew.to_string(),
            segments,
        })
    }
}

impl serde::Serialize for Codename {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

impl<'de> serde::Deserialize<'de> for Codename {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_every_shape() {
        for s in [
            "cobalt-harbor",
            "cobalt-harbor/heron",
            "cobalt-harbor/heron#412",
            "cobalt-harbor/heron#412.2",
            "cobalt-harbor/heron#412>lynx#3",
            "cobalt-harbor/heron#412>lynx#3>otter#1",
            "saffron-ridge/heron>lynx#5",
        ] {
            let c: Codename = s.parse().unwrap();
            assert_eq!(c.to_string(), s);
        }
    }

    #[test]
    fn leaf_drops_the_crew() {
        let c: Codename = "cobalt-harbor/heron#412>lynx#3".parse().unwrap();
        assert_eq!(c.leaf(), "heron#412>lynx#3");
        assert_eq!(Codename::crew_only("cobalt-harbor").leaf(), "cobalt-harbor");
    }

    #[test]
    fn builders() {
        let c = Codename::crew_only("cobalt-harbor")
            .child("heron", Some(412))
            .with_attempt(2);
        assert_eq!(c.to_string(), "cobalt-harbor/heron#412.2");
        assert_eq!(
            c.child("lynx", Some(1)).to_string(),
            "cobalt-harbor/heron#412.2>lynx#1"
        );
    }

    #[test]
    fn rejects_non_codenames() {
        for s in [
            "run_01J9ZQ",
            "01J9ZQ3K",
            "cobalt",
            "Cobalt-harbor",
            "cobalt-harbor/",
            "cobalt-harbor/heron#",
            "cobalt-harbor/heron#0",
            "a-b/c>>d",
        ] {
            assert!(s.parse::<Codename>().is_err(), "{s} should not parse");
        }
    }

    #[test]
    fn serde_is_the_string_form() {
        let c: Codename = "cobalt-harbor/heron#4".parse().unwrap();
        assert_eq!(
            serde_json::to_string(&c).unwrap(),
            "\"cobalt-harbor/heron#4\""
        );
        let back: Codename = serde_json::from_str("\"cobalt-harbor/heron#4\"").unwrap();
        assert_eq!(back, c);
    }
}
