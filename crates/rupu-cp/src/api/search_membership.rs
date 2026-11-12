//! Search-only membership capture. Never projects legacy transcript bodies.
use super::*;
use serde::Serialize;
use std::sync::atomic::{AtomicBool, Ordering};

pub(crate) const MAX_MEMBERS: usize = 10_000;
#[derive(Clone, Serialize)]
pub(crate) struct Member {
    pub run_id: String,
    pub parent: Option<usize>,
    pub source: &'static str,
    pub path: Option<PathBuf>,
    pub lead_round: Option<u32>,
}
pub(crate) struct Captured {
    pub proof: RootProof,
    pub members: Vec<Member>,
    pub malformed: usize,
    pub incomplete: usize,
    pub remote: usize,
}
pub(crate) fn cancelled(cancel: &AtomicBool) -> ApiResult<()> {
    if cancel.load(Ordering::Relaxed) {
        Err(ApiError::conflict("transcript search cancelled"))
    } else {
        Ok(())
    }
}
fn add(
    rows: &mut Vec<Member>,
    seen: &mut BTreeMap<String, usize>,
    row: Member,
    weight: &mut usize,
) -> ApiResult<()> {
    valid_id(&row.run_id)?;
    if let Some(&prior) = seen.get(&row.run_id) {
        let old = &rows[prior];
        if old.parent != row.parent || old.path != row.path {
            return Err(ApiError::conflict(
                "search membership has conflicting parent or transcript records",
            ));
        }
        return Ok(());
    }
    if rows.len() >= MAX_MEMBERS {
        return Err(too_large("search exceeds 10000 recorded members"));
    }
    if row
        .path
        .as_ref()
        .is_some_and(|p| p.as_os_str().len() > 8192 || !p.is_absolute())
    {
        return Err(ApiError::conflict("recorded transcript locator is invalid"));
    }
    *weight = weight.saturating_add(
        serde_json::to_vec(&row)
            .map_err(|e| ApiError::internal(e.to_string()))?
            .len()
            + 1024,
    );
    if *weight > 16 * 1024 * 1024 {
        return Err(too_large("search member metadata exceeds 16 MiB"));
    }
    seen.insert(row.run_id.clone(), rows.len());
    rows.push(row);
    Ok(())
}
pub(crate) fn capture(owner: Owner, id: &str, cancel: &AtomicBool) -> ApiResult<Captured> {
    cancelled(cancel)?;
    valid_id(id)?;
    let stores = child_stores(&owner)?;
    let (mut reader, proof) = root::resolve(&owner, id)?;
    let agentiflow = proof.kind == "agentiflow";
    let root_store = reader.root.clone();
    let mut members = vec![Member {
        run_id: id.into(),
        parent: None,
        source: "root",
        path: proof.transcript_path.clone(),
        lead_round: agentiflow.then_some(0),
    }];
    let mut seen = BTreeMap::from([(id.to_owned(), 0)]);
    let mut weight = 65536;
    let mut index = 0;
    let mut remote = 0;
    while index < members.len() {
        cancelled(cancel)?;
        let run = members[index].run_id.clone();
        let selected = if index == 0 {
            reader.root = root_store.clone();
            reader
                .checked_metadata(&root_store.join(id))?
                .map(|m| (root_store.join(id), Identity::of(&m)))
        } else {
            select_child_store(&mut reader, &stores, &run)?
        };
        let directory = reader.root.join(&run);
        // Step result membership includes nested workflows and fan-out units;
        // explicit remote records never resolve against this machine's paths.
        if let Some(text) = if agentiflow && index == 0 {
            None
        } else {
            reader.lines(&directory.join("step_results.jsonl"))?
        } {
            for line in text.lines().filter(|l| !l.trim().is_empty()) {
                cancelled(cancel)?;
                match serde_json::from_str::<StepResultRecord>(line) {
                    Ok(record) => {
                        let local = record.host.as_deref().is_none_or(|h| h == "local");
                        let mut candidates = vec![(record.run_id, record.transcript_path)];
                        candidates.extend(
                            record
                                .items
                                .into_iter()
                                .map(|r| (r.run_id, r.transcript_path)),
                        );
                        for (run_id, path) in candidates {
                            if run_id.is_empty() {
                                continue;
                            }
                            if !local {
                                remote += 1;
                                continue;
                            }
                            add(
                                &mut members,
                                &mut seen,
                                Member {
                                    run_id,
                                    parent: Some(index),
                                    source: "workflow_step",
                                    lead_round: None,
                                    path: (!path.as_os_str().is_empty()).then_some(path),
                                },
                                &mut weight,
                            )?;
                        }
                    }
                    Err(_) => reader.evidence.malformed_rows += 1,
                }
            }
        }
        let text = reader
            .lines(&directory.join("children.jsonl"))?
            .unwrap_or_default();
        let mut known = BTreeSet::new();
        // Started rows are authoritative historical edges. Folding lifecycle
        // state is unnecessary for search and can hide an earlier transcript.
        for line in text.lines().filter(|l| !l.trim().is_empty()) {
            cancelled(cancel)?;
            match serde_json::from_str::<LedgerLine>(line) {
                Ok(LedgerLine::Started(row)) => {
                    if agentiflow && index == 0 {
                        root::validate_agentiflow_edge(&row, &directory)?;
                    }
                    known.insert(row.run_id.clone());
                    add(
                        &mut members,
                        &mut seen,
                        Member {
                            run_id: row.run_id,
                            parent: Some(index),
                            source: "children_ledger",
                            lead_round: None,
                            path: row.transcript,
                        },
                        &mut weight,
                    )?;
                }
                Ok(_) => {}
                Err(_) => reader.evidence.malformed_rows += 1,
            }
        }
        let legacy = if agentiflow && index == 0 {
            Vec::new()
        } else {
            reader.legacy_ids(&run)?
        };
        for run_id in legacy {
            cancelled(cancel)?;
            if known.contains(&run_id) {
                continue;
            }
            let path = directory.join("sub").join(&run_id).join("transcript.jsonl");
            add(
                &mut members,
                &mut seen,
                Member {
                    run_id,
                    parent: Some(index),
                    source: "legacy_subdirectory",
                    lead_round: None,
                    path: Some(path),
                },
                &mut weight,
            )?;
        }
        validate_child_directory(selected.as_ref())?;
        index += 1;
    }
    if agentiflow {
        reader.root = root_store.clone();
        append_lead_rounds(&mut reader, id, &mut members, &mut weight, cancel)?;
    }
    proof.validate_search()?;
    Ok(Captured {
        proof,
        members,
        malformed: reader.evidence.malformed_rows,
        incomplete: reader.evidence.incomplete_tail_bytes,
        remote,
    })
}

// Every lead round belongs to the same flow RunStart identity. Its exact path is
// a distinct searchable transcript, never a fabricated descendant run ID.
fn append_lead_rounds(
    reader: &mut Reader,
    id: &str,
    members: &mut Vec<Member>,
    weight: &mut usize,
    cancel: &AtomicBool,
) -> ApiResult<()> {
    let directory = reader.root.join(id).join("lead");
    let before = reader
        .checked_metadata(&directory)?
        .ok_or_else(|| stale("lead directory disappeared"))?;
    if !before.is_dir() {
        return Err(stale("lead source is not a directory"));
    }
    let mut rounds = BTreeMap::new();
    for entry in fs::read_dir(&directory).map_err(|e| ApiError::internal(e.to_string()))? {
        cancelled(cancel)?;
        reader.evidence.directory_entries_scanned += 1;
        if reader.evidence.directory_entries_scanned > MAX_ENTRIES {
            return Err(too_large("lead directory entry budget exceeded"));
        }
        let entry = entry.map_err(|e| ApiError::internal(e.to_string()))?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        let Some(number) = name
            .strip_prefix("transcript.r")
            .and_then(|s| s.strip_suffix(".jsonl"))
        else {
            continue;
        };
        let round: u32 = number
            .parse()
            .map_err(|_| ApiError::conflict("invalid recorded lead round"))?;
        if number != round.to_string() {
            return Err(ApiError::conflict("noncanonical recorded lead round"));
        }
        let metadata = reader
            .checked_metadata(&entry.path())?
            .ok_or_else(|| stale("lead round disappeared"))?;
        if !metadata.is_file() {
            return Err(ApiError::conflict("lead transcript is not regular"));
        }
        if rounds.insert(round, entry.path()).is_some() {
            return Err(ApiError::conflict("duplicate recorded lead round"));
        }
        if rounds.len().saturating_sub(1).saturating_add(members.len()) > MAX_MEMBERS {
            return Err(too_large("search exceeds 10000 recorded transcripts"));
        }
    }
    for (round, path) in rounds {
        if round == 0 {
            continue;
        }
        let member = Member {
            run_id: id.into(),
            parent: None,
            source: "agentiflow_lead",
            path: Some(path),
            lead_round: Some(round),
        };
        *weight = weight.saturating_add(
            serde_json::to_vec(&member)
                .map_err(|e| ApiError::internal(e.to_string()))?
                .len()
                + 1024,
        );
        if *weight > 16 * 1024 * 1024 {
            return Err(too_large("search member metadata exceeds 16 MiB"));
        }
        members.push(member);
    }
    let after = reader
        .checked_metadata(&directory)?
        .ok_or_else(|| stale("lead directory disappeared"))?;
    if Identity::of(&before) != Identity::of(&after) {
        return Err(stale("lead directory replaced during capture"));
    }
    Ok(())
}
