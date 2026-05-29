//! XML parsing layer. Reads MITRE's CWE XML into intermediate
//! `RawWeakness` and `RawView` structs that the mapper layer (Task 13)
//! transforms into our Concern type.

#[allow(unused_imports)] // used in Task 12 state machine
use quick_xml::events::Event;
#[allow(unused_imports)] // used in Task 12 state machine
use quick_xml::reader::Reader;
use std::path::Path;

#[derive(Debug, Clone)]
pub struct RawWeakness {
    pub id: u32,
    pub name: String,
    pub description: String,
    pub extended_description: Option<String>,
    /// E.g. `["Modify Memory"]`. Empty when MITRE doesn't classify.
    pub impact_tags: Vec<String>,
    /// E.g. `["C", "C++"]`. Empty when language-agnostic.
    pub applicable_languages: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct RawView {
    pub id: u32,
    pub name: String,
    /// Weakness IDs that are members of this view (transitively through
    /// categories — already flattened).
    pub member_weakness_ids: Vec<u32>,
}

#[derive(Debug)]
pub struct ParsedCwe {
    pub weaknesses: Vec<RawWeakness>,
    pub views: Vec<RawView>,
}

#[derive(Debug, thiserror::Error)]
pub enum CweXmlError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("xml: {0}")]
    Xml(#[from] quick_xml::Error),
    #[error("malformed: {0}")]
    Malformed(String),
}

pub fn parse_cwe_xml(path: &Path) -> Result<ParsedCwe, CweXmlError> {
    let xml = std::fs::read_to_string(path)?;
    parse_cwe_xml_str(&xml)
}

pub fn parse_cwe_xml_str(_xml: &str) -> Result<ParsedCwe, CweXmlError> {
    // Skeleton: the real event-driven parser is implemented in Task 12,
    // which replaces this body. For now, return an empty result so the
    // module compiles and the smoke test can call it.
    Ok(ParsedCwe {
        weaknesses: Vec::new(),
        views: Vec::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const TINY_FIXTURE: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<Weakness_Catalog>
  <Weaknesses>
    <Weakness ID="787" Name="Out-of-bounds Write">
      <Description>The code writes data past the end of the intended buffer.</Description>
      <Extended_Description>This typically occurs when a pointer or index is incremented beyond bounds.</Extended_Description>
      <Common_Consequences>
        <Consequence>
          <Scope>Integrity</Scope>
          <Impact>Modify Memory</Impact>
        </Consequence>
      </Common_Consequences>
      <Applicable_Platforms>
        <Language Name="C" />
        <Language Name="C++" />
      </Applicable_Platforms>
    </Weakness>
  </Weaknesses>
  <Views>
    <View ID="699" Name="Software Development">
      <Members>
        <Has_Member CWE_ID="787" />
      </Members>
    </View>
  </Views>
</Weakness_Catalog>
"#;

    #[test]
    fn parses_minimal_fixture_without_panicking() {
        // Skeleton just verifies the parser runs cleanly on a tiny fixture.
        // Full parsing is implemented in Task 12 (which replaces the
        // parse_cwe_xml_str body and these assertions).
        let parsed = parse_cwe_xml_str(TINY_FIXTURE).expect("parses");
        let _ = parsed.weaknesses.len();
        let _ = parsed.views.len();
    }
}
