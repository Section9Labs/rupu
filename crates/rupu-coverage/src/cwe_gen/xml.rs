//! XML parsing layer. Reads MITRE's CWE XML into intermediate
//! `RawWeakness` and `RawView` structs that the mapper layer (Task 13)
//! transforms into our Concern type.

use quick_xml::events::Event;
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

pub fn parse_cwe_xml_str(xml: &str) -> Result<ParsedCwe, CweXmlError> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);

    let mut weaknesses: Vec<RawWeakness> = Vec::new();
    let mut views_raw: Vec<RawView> = Vec::new();
    // category id -> list of weakness ids it contains
    let mut categories: std::collections::HashMap<u32, Vec<u32>> =
        std::collections::HashMap::new();

    // current-context tracking
    let mut cur_weakness: Option<RawWeakness> = None;
    let mut cur_category: Option<(u32, Vec<u32>)> = None;
    let mut cur_view: Option<RawView> = None;
    let mut in_description = false;
    let mut in_extended = false;
    let mut in_impact = false;
    let mut cur_impact_text = String::new();

    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf)? {
            Event::Start(ref e) => {
                let name = elem_local_name(e);
                match name.as_str() {
                    "Weakness" => {
                        cur_weakness = Some(RawWeakness {
                            id: attr_u32(e, b"ID", &reader)?.unwrap_or(0),
                            name: attr_string(e, b"Name", &reader)?.unwrap_or_default(),
                            description: String::new(),
                            extended_description: None,
                            impact_tags: Vec::new(),
                            applicable_languages: Vec::new(),
                        });
                    }
                    "Description" if cur_weakness.is_some() => in_description = true,
                    "Extended_Description" if cur_weakness.is_some() => in_extended = true,
                    "Impact" if cur_weakness.is_some() => in_impact = true,
                    "Category" => {
                        let id = attr_u32(e, b"ID", &reader)?.unwrap_or(0);
                        cur_category = Some((id, Vec::new()));
                    }
                    "View" => {
                        cur_view = Some(RawView {
                            id: attr_u32(e, b"ID", &reader)?.unwrap_or(0),
                            name: attr_string(e, b"Name", &reader)?.unwrap_or_default(),
                            member_weakness_ids: Vec::new(),
                        });
                    }
                    _ => {}
                }
            }
            Event::Text(ref e) => {
                let text = e.unescape()?.into_owned();
                if let Some(ref mut w) = cur_weakness {
                    if in_description {
                        w.description.push_str(&text);
                    } else if in_extended {
                        w.extended_description
                            .get_or_insert_with(String::new)
                            .push_str(&text);
                    } else if in_impact {
                        cur_impact_text.push_str(&text);
                    }
                }
            }
            Event::Empty(ref e) => {
                let name = elem_local_name(e);
                match name.as_str() {
                    "Language" => {
                        if let Some(ref mut w) = cur_weakness {
                            if let Some(lang) = attr_string(e, b"Name", &reader)? {
                                w.applicable_languages.push(lang);
                            }
                        }
                    }
                    "Has_Member" => {
                        if let Some(id) = attr_u32(e, b"CWE_ID", &reader)? {
                            if let Some((_, ref mut members)) = cur_category {
                                members.push(id);
                            } else if let Some(ref mut v) = cur_view {
                                v.member_weakness_ids.push(id);
                            }
                        }
                    }
                    _ => {}
                }
            }
            Event::End(ref e) => {
                let name = end_local_name(e);
                match name.as_str() {
                    "Weakness" => {
                        if let Some(mut w) = cur_weakness.take() {
                            w.description = w.description.trim().to_string();
                            if let Some(ref mut ed) = w.extended_description {
                                *ed = ed.trim().to_string();
                                if ed.is_empty() {
                                    w.extended_description = None;
                                }
                            }
                            weaknesses.push(w);
                        }
                    }
                    "Description" if in_description => in_description = false,
                    "Extended_Description" if in_extended => in_extended = false,
                    "Impact" if in_impact => {
                        in_impact = false;
                        let accumulated = cur_impact_text.trim().to_string();
                        if !accumulated.is_empty() {
                            if let Some(ref mut w) = cur_weakness {
                                w.impact_tags.push(accumulated);
                            }
                        }
                        cur_impact_text.clear();
                    }
                    "Category" => {
                        if let Some((id, members)) = cur_category.take() {
                            categories.insert(id, members);
                        }
                    }
                    "View" => {
                        if let Some(v) = cur_view.take() {
                            views_raw.push(v);
                        }
                    }
                    _ => {}
                }
            }
            Event::Eof => break,
            _ => {}
        }
        buf.clear();
    }

    // Flatten view membership: iteratively expand category refs until only weakness IDs remain.
    let views = views_raw
        .into_iter()
        .map(|mut v| {
            let mut resolved: std::collections::BTreeSet<u32> = std::collections::BTreeSet::new();
            let mut pending: Vec<u32> = std::mem::take(&mut v.member_weakness_ids);
            let mut visited_categories: std::collections::HashSet<u32> =
                std::collections::HashSet::new();
            while let Some(id) = pending.pop() {
                if let Some(cat_members) = categories.get(&id) {
                    // `id` is a category — expand its members and keep resolving.
                    // Guard against (pathological) category cycles.
                    if visited_categories.insert(id) {
                        pending.extend(cat_members.iter().copied());
                    }
                } else {
                    // `id` is a weakness (not present in the categories map).
                    resolved.insert(id);
                }
            }
            v.member_weakness_ids = resolved.into_iter().collect();
            v
        })
        .collect();

    Ok(ParsedCwe { weaknesses, views })
}

/// Extract the local name (without namespace prefix) from a `BytesStart`.
fn elem_local_name(e: &quick_xml::events::BytesStart<'_>) -> String {
    String::from_utf8_lossy(e.name().local_name().as_ref()).into_owned()
}

/// Extract the local name from a `BytesEnd`.
fn end_local_name(e: &quick_xml::events::BytesEnd<'_>) -> String {
    String::from_utf8_lossy(e.name().local_name().as_ref()).into_owned()
}

/// Find an attribute by byte-string key and return its decoded string value,
/// or `None` if the attribute is absent.
fn attr_string(
    e: &quick_xml::events::BytesStart<'_>,
    key: &[u8],
    reader: &Reader<&[u8]>,
) -> Result<Option<String>, CweXmlError> {
    let decoder = reader.decoder();
    for attr in e.attributes() {
        let attr = attr.map_err(quick_xml::Error::from)?;
        if attr.key.as_ref() == key {
            let val = attr.decode_and_unescape_value(decoder)?;
            return Ok(Some(val.into_owned()));
        }
    }
    Ok(None)
}

/// Find an attribute by byte-string key and parse it as `u32`.
fn attr_u32(
    e: &quick_xml::events::BytesStart<'_>,
    key: &[u8],
    reader: &Reader<&[u8]>,
) -> Result<Option<u32>, CweXmlError> {
    match attr_string(e, key, reader)? {
        None => Ok(None),
        Some(s) => s
            .parse::<u32>()
            .map(Some)
            .map_err(|_| {
                CweXmlError::Malformed(format!(
                    "expected u32 for {:?}, got {s:?}",
                    String::from_utf8_lossy(key)
                ))
            }),
    }
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
    fn parses_minimal_fixture() {
        let parsed = parse_cwe_xml_str(TINY_FIXTURE).expect("parses");
        assert_eq!(parsed.weaknesses.len(), 1);
        assert_eq!(parsed.weaknesses[0].id, 787);
        assert_eq!(parsed.weaknesses[0].name, "Out-of-bounds Write");
        assert!(parsed.weaknesses[0].description.contains("writes data past the end"));
        assert_eq!(parsed.weaknesses[0].applicable_languages, vec!["C", "C++"]);
        assert_eq!(parsed.weaknesses[0].impact_tags, vec!["Modify Memory"]);

        assert_eq!(parsed.views.len(), 1);
        assert_eq!(parsed.views[0].id, 699);
        assert_eq!(parsed.views[0].member_weakness_ids, vec![787]);
    }

    const FIXTURE_WITH_CATEGORY: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<Weakness_Catalog>
  <Weaknesses>
    <Weakness ID="787" Name="OOB Write">
      <Description>desc</Description>
    </Weakness>
    <Weakness ID="125" Name="OOB Read">
      <Description>desc</Description>
    </Weakness>
  </Weaknesses>
  <Categories>
    <Category ID="119" Name="Memory Buffer Errors">
      <Relationships>
        <Has_Member CWE_ID="787" View_ID="1000" />
        <Has_Member CWE_ID="125" View_ID="1000" />
      </Relationships>
    </Category>
  </Categories>
  <Views>
    <View ID="1000" Name="Research">
      <Members>
        <Has_Member CWE_ID="119" View_ID="1000" />
      </Members>
    </View>
  </Views>
</Weakness_Catalog>
"#;

    #[test]
    fn view_expands_category_members() {
        let parsed = parse_cwe_xml_str(FIXTURE_WITH_CATEGORY).expect("parses");
        let view = &parsed.views[0];
        // The view referenced category 119, which contains 787 and 125.
        assert_eq!(view.member_weakness_ids, vec![125, 787]); // sorted by BTreeSet
    }

    const FIXTURE_NESTED_CATEGORIES: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<Weakness_Catalog>
  <Weaknesses>
    <Weakness ID="400" Name="Uncontrolled Resource Consumption">
      <Description>desc</Description>
    </Weakness>
    <Weakness ID="770" Name="Allocation Without Limits">
      <Description>desc</Description>
    </Weakness>
  </Weaknesses>
  <Categories>
    <Category ID="664" Name="Pillar">
      <Relationships>
        <Has_Member CWE_ID="399" View_ID="1000" />
      </Relationships>
    </Category>
    <Category ID="399" Name="Resource Management">
      <Relationships>
        <Has_Member CWE_ID="400" View_ID="1000" />
        <Has_Member CWE_ID="770" View_ID="1000" />
      </Relationships>
    </Category>
  </Categories>
  <Views>
    <View ID="1000" Name="Research">
      <Members>
        <Has_Member CWE_ID="664" View_ID="1000" />
      </Members>
    </View>
  </Views>
</Weakness_Catalog>
"#;

    #[test]
    fn view_expands_nested_categories_to_all_weaknesses() {
        let parsed = parse_cwe_xml_str(FIXTURE_NESTED_CATEGORIES).expect("parses");
        let view = &parsed.views[0];
        // View 1000 → category 664 → category 399 → weaknesses 400, 770.
        // The one-level expansion would have yielded [399] (a category id);
        // the fix must yield the actual weakness ids.
        assert_eq!(view.member_weakness_ids, vec![400, 770]);
    }
}
