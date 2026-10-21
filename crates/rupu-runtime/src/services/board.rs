//! The board as an operator reads it: claims (from the claim journal) and
//! directives (with retractions kept, not folded away). The wire shape of the
//! CP's board endpoint, `rupu __board` and the MCP tool.

use chrono::{DateTime, Utc};
use rupu_fleet::{Board, ClaimView, DirectiveEvent, FleetError};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DirectiveView {
    pub id: String,
    pub author: String,
    pub ts: String,
    pub body: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub addressed_to: Option<String>,
    /// When it was retracted; absent for a live directive and for a
    /// retraction written before retractions carried a time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retracted_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub retracted: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BoardView {
    pub claims: Vec<ClaimView>,
    pub directives: Vec<DirectiveView>,
    /// Whether the space keeps a claim journal; false for a space that
    /// predates it, whose `claims` then hold only what is live now.
    pub journal: bool,
}

/// The board of the message space at `space`. An empty or absent board is an
/// empty view, not an error.
pub fn board_view(space: &Path, now: DateTime<Utc>) -> Result<BoardView, FleetError> {
    let board = Board::new(space);
    let claims = board.claims_view(now)?;
    let mut directives: Vec<DirectiveView> = Vec::new();
    let mut slot_of: HashMap<String, usize> = HashMap::new();
    for event in board.directive_events()? {
        match event {
            DirectiveEvent::Put(d) => {
                let view = DirectiveView {
                    id: d.id.clone(),
                    author: d.author,
                    ts: d.ts,
                    body: d.body,
                    addressed_to: d.addressed_to,
                    retracted_at: None,
                    retracted: false,
                };
                match slot_of.get(&d.id) {
                    Some(&slot) if !d.id.trim().is_empty() => directives[slot] = view,
                    _ => {
                        if !d.id.trim().is_empty() {
                            slot_of.insert(d.id, directives.len());
                        }
                        directives.push(view);
                    }
                }
            }
            DirectiveEvent::Retract { retract, at } => {
                if let Some(&slot) = slot_of.get(&retract) {
                    let v = &mut directives[slot];
                    v.retracted = true;
                    v.retracted_at = at
                        .as_deref()
                        .and_then(|t| DateTime::parse_from_rfc3339(t).ok())
                        .map(|t| t.with_timezone(&Utc));
                }
            }
        }
    }
    Ok(BoardView {
        claims,
        directives,
        journal: board.has_journal(),
    })
}
