//! The skills events' wire names, pinned against the JSON examples the
//! transcript schema reference prints, read from the doc itself so a rename
//! on either side fails here.

use rupu_transcript::event::{Event, SkillIndexEntry};

const SCHEMA_DOC: &str = include_str!("../../../../docs/transcript-schema.md");

fn doc_example(event_type: &str) -> Event {
    let prefix = format!("{{\"type\":\"{event_type}\"");
    let line = SCHEMA_DOC
        .lines()
        .find(|l| l.starts_with(&prefix))
        .unwrap_or_else(|| panic!("no {event_type} example in docs/transcript-schema.md"));
    serde_json::from_str(line).expect("the doc example deserializes")
}

#[test]
fn skill_index_doc_example() {
    assert_eq!(
        doc_example("skill_index"),
        Event::SkillIndex {
            skills: vec![SkillIndexEntry {
                name: "pdf".into(),
                layer: "global".into(),
                stamp: "9f2c…".into(),
                preload: false,
                indexed: true,
                reason: "agent".into(),
            }],
        }
    );
}

#[test]
fn skill_load_doc_example() {
    assert_eq!(
        doc_example("skill_load"),
        Event::SkillLoad {
            name: "pdf".into(),
            layer: "global".into(),
            content_hash: "4b1e…".into(),
            base_dir: "/home/me/.rupu/skills/pdf".into(),
            via: "model".into(),
        }
    );
}
