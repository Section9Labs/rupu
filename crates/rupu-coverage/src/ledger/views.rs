use crate::catalog::types::TouchStrength;
use crate::ledger::events::{Attribution, ConcernAssertion, FileTouchEvent};
use crate::ledger::paths::CoveragePaths;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileView {
    pub path: String,
    pub touch_modes: Vec<TouchStrength>,
    pub strongest: TouchStrength,
    pub read_lines: Vec<[u32; 2]>,
    pub grep_matches: u32,
    pub edits: u32,
    pub first_at: DateTime<Utc>,
    pub last_at: DateTime<Utc>,
    pub touched_by: Vec<Attribution>,
}

pub fn read_file_events(paths: &CoveragePaths) -> std::io::Result<Vec<FileTouchEvent>> {
    if !paths.files.exists() {
        return Ok(vec![]);
    }
    let raw = std::fs::read_to_string(&paths.files)?;
    Ok(raw
        .lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| serde_json::from_str::<FileTouchEvent>(l).ok())
        .collect())
}

pub fn read_concern_assertions(paths: &CoveragePaths) -> std::io::Result<Vec<ConcernAssertion>> {
    if !paths.concerns.exists() {
        return Ok(vec![]);
    }
    let raw = std::fs::read_to_string(&paths.concerns)?;
    Ok(raw
        .lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| serde_json::from_str::<ConcernAssertion>(l).ok())
        .collect())
}

pub fn file_views(events: &[FileTouchEvent]) -> Vec<FileView> {
    let mut by_path: BTreeMap<String, FileView> = BTreeMap::new();
    for ev in events {
        let Some(path) = ev.path() else { continue };
        let Some(strength) = ev.strength() else { continue };
        let at = match ev {
            FileTouchEvent::Read { at, .. }
            | FileTouchEvent::Grep { at, .. }
            | FileTouchEvent::Glob { at, .. }
            | FileTouchEvent::Edit { at, .. }
            | FileTouchEvent::Cmd { at, .. }
            | FileTouchEvent::Unknown { at, .. } => *at,
        };
        let attribution = match ev {
            FileTouchEvent::Read { attribution, .. }
            | FileTouchEvent::Grep { attribution, .. }
            | FileTouchEvent::Glob { attribution, .. }
            | FileTouchEvent::Edit { attribution, .. }
            | FileTouchEvent::Cmd { attribution, .. }
            | FileTouchEvent::Unknown { attribution, .. } => attribution.clone(),
        };
        let view = by_path.entry(path.to_string()).or_insert_with(|| FileView {
            path: path.to_string(),
            touch_modes: vec![],
            strongest: strength,
            read_lines: vec![],
            grep_matches: 0,
            edits: 0,
            first_at: at,
            last_at: at,
            touched_by: vec![],
        });
        if !view.touch_modes.contains(&strength) {
            view.touch_modes.push(strength);
        }
        if strength > view.strongest {
            view.strongest = strength;
        }
        if at < view.first_at {
            view.first_at = at;
        }
        if at > view.last_at {
            view.last_at = at;
        }
        if !view.touched_by.iter().any(|a| a == &attribution) {
            view.touched_by.push(attribution);
        }
        match ev {
            FileTouchEvent::Read { line_range, .. } => view.read_lines.push(*line_range),
            FileTouchEvent::Edit { .. } => view.edits += 1,
            FileTouchEvent::Grep { match_count, .. } => view.grep_matches += match_count,
            _ => {}
        }
    }
    by_path.into_values().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ledger::events::Surface;

    fn attribution() -> Attribution {
        Attribution {
            run_id: "run_t".to_string(),
            model: "m".to_string(),
            surface: Surface::Workflow,
        }
    }

    #[test]
    fn file_views_aggregates_multiple_touches_per_path() {
        let now = Utc::now();
        let events = vec![
            FileTouchEvent::Read {
                path: "src/a.rs".to_string(),
                line_range: [1, 100],
                tool: "read_file".to_string(),
                attribution: attribution(),
                at: now,
            },
            FileTouchEvent::Read {
                path: "src/a.rs".to_string(),
                line_range: [101, 200],
                tool: "read_file".to_string(),
                attribution: attribution(),
                at: now,
            },
            FileTouchEvent::Edit {
                path: "src/a.rs".to_string(),
                line_range: [50, 55],
                lines_changed: 5,
                tool: "edit_file".to_string(),
                attribution: attribution(),
                at: now,
            },
        ];
        let views = file_views(&events);
        assert_eq!(views.len(), 1);
        let v = &views[0];
        assert_eq!(v.path, "src/a.rs");
        assert_eq!(v.strongest, TouchStrength::Edit);
        assert_eq!(v.read_lines.len(), 2);
        assert_eq!(v.edits, 1);
    }
}
