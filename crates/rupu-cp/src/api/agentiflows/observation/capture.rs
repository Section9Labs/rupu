use super::input::Input;
use super::*;
use serde_json::json;
use sha2::Digest;
use std::path::Path;

const MAX_LINE: usize = 256 * 1024;
const MAX_ROWS: usize = 100_000;
const MAX_CAPTURE: usize = 64 * 1024 * 1024;

fn envelope(id: String, source: &str, value: Value) -> Value {
    json!({"id": id, "source": source, "value": value})
}
fn safe_component(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}
fn bounded_output(mut unit: Value) -> Value {
    let present = unit["status"]["output"]
        .as_str()
        .is_some_and(|s| !s.is_empty());
    unit["status"]["output"] = Value::Null;
    unit["output_recorded_in_capture"] = json!(present);
    unit
}

pub(super) fn capture(s: &AppState, id: &str) -> ApiResult<Captured> {
    let root = existing_run_dir(s, id)?;
    let mut input = Input::new(&s.global_dir, id)?;
    let record = input
        .copy(Path::new("agentiflow.json"), MAX_LINE)?
        .ok_or_else(|| ApiError::not_found("Agentiflow record unavailable"))?;
    let record: AgentiflowRecord = serde_json::from_slice(&record)
        .map_err(|_| ApiError::conflict("Agentiflow record is malformed"))?;
    if record.id != id {
        return Err(ApiError::conflict(
            "Agentiflow record identity differs from directory",
        ));
    }
    let definition_bytes = input.copy(Path::new("agentiflow.yaml"), MAX_LINE)?;
    let definition = definition_bytes
        .as_deref()
        .and_then(|v| std::str::from_utf8(v).ok())
        .and_then(|v| AgentiflowDef::parse_str(v).ok())
        .map(def_dto);
    let definition_status = if definition.is_some() {
        "loaded"
    } else if definition_bytes.is_some() {
        "malformed"
    } else {
        "missing"
    };
    let mut sections: BTreeMap<String, Vec<Value>> = ["units", "events", "board", "leads"]
        .into_iter()
        .map(|s| (s.into(), Vec::new()))
        .collect();
    let event_bytes = input
        .copy(Path::new("events.jsonl"), super::input::MAX_FILE)?
        .unwrap_or_default();
    let events = input.lines(&event_bytes)?;
    let budget = events
        .iter()
        .rev()
        .find_map(|row| {
            (row["value"]["kind"].as_str() == Some("round"))
                .then(|| row["value"]["budget"].as_str())
                .flatten()
        })
        .map(str::to_owned);
    for event in events {
        sections.get_mut("events").unwrap().push(envelope(
            format!("event:{}", event["line"]),
            "coordinator",
            event["value"].clone(),
        ));
    }
    // Existing folding runs only over a private, size-bounded mirror. No live
    // ledger is reopened while a page is requested.
    for row in input.jsonl("children.jsonl")? {
        if serde_json::from_value::<rupu_launch::ledger::LedgerLine>(row["value"].clone()).is_err()
        {
            input.malformed += 1;
        }
    }
    let mut units = BTreeMap::new();
    for entry in input.entries(Path::new("units"))? {
        if !entry.is_dir {
            continue;
        }
        let name = entry.name.to_string_lossy().into_owned();
        if !safe_component(&name) {
            input.malformed += 1;
            continue;
        }
        let rel = PathBuf::from("units").join(&name).join("unit.json");
        let Some(bytes) = input.copy(&rel, MAX_LINE)? else {
            continue;
        };
        match serde_json::from_slice::<Value>(&bytes) {
            Ok(raw)
                if raw["status"]["state"]
                    .as_str()
                    .is_some_and(|s| ["pending", "running", "done", "failed"].contains(&s)) =>
            {
                let agent = raw["agent"].as_str();
                let (codename, derived) = crate::codename::named(None, &name, agent);
                let value = json!({"unit_id": name, "codename": codename, "codename_derived": derived,
                    "agent": raw["agent"], "participant": raw["participant"], "kind": raw["kind"].as_str().unwrap_or("agent"),
                    "status": raw["status"], "pgid": raw["pgid"], "started_at": raw["started_at"],
                    "transcript_path": unit_transcript(s, &name), "round": raw["round"], "generated": false});
                units.insert(
                    name,
                    envelope(
                        format!("unit:{}", value["unit_id"].as_str().unwrap()),
                        "legacy_unit",
                        bounded_output(value),
                    ),
                );
            }
            _ => input.malformed += 1,
        }
    }
    for child in rupu_launch::ledger::read(input.mirror.path()) {
        if !safe_component(&child.started.run_id) {
            input.malformed += 1;
            continue;
        }
        let handle = child.started.handle.clone();
        let value = serde_json::to_value(ledger_unit(s, child))
            .map_err(|_| ApiError::internal("cannot project captured unit"))?;
        units.insert(
            value["unit_id"].as_str().unwrap().to_owned(),
            envelope(
                format!("child:{handle}"),
                "children_ledger",
                bounded_output(value),
            ),
        );
    }
    sections.insert("units".into(), units.into_values().collect());
    let mut leads = Vec::new();
    for entry in input.entries(Path::new("lead"))? {
        if !entry.is_file {
            continue;
        }
        let name = entry.name.to_string_lossy().into_owned();
        if let Some(round) = name
            .strip_prefix("transcript.r")
            .and_then(|s| s.strip_suffix(".jsonl"))
            .and_then(|s| s.parse::<u32>().ok())
        {
            input.hash.update(name.as_bytes());
            leads.push((round, root.join("lead").join(&entry.name)));
        }
    }
    leads.sort_by_key(|(round, _)| *round);
    if leads.windows(2).any(|pair| pair[0].0 == pair[1].0) {
        return Err(ApiError::conflict(
            "multiple transcript handles name one lead round",
        ));
    }
    let latest_round = leads.last().map(|(round, _)| *round);
    for (round, path) in leads {
        let mut value = json!({"round": round, "path": path});
        if Some(round) == latest_round {
            let relative = path
                .strip_prefix(&root)
                .map_err(|_| ApiError::internal("invalid captured lead path"))?;
            let (mut status, line) = input.first_line(relative)?;
            let mut event = Value::Null;
            if let Some(line) = line {
                if let (
                    Ok(rupu_transcript::Event::RunStart {
                        run_id,
                        workspace_id,
                        ..
                    }),
                    Ok(raw),
                ) = (
                    serde_json::from_slice::<rupu_transcript::Event>(&line),
                    serde_json::from_slice::<Value>(&line),
                ) {
                    let hosts_match = [&raw["host_id"], &raw["data"]["host_id"]]
                        .iter()
                        .all(|v| v.is_null() || v.as_str() == Some("local"));
                    let run_matches =
                        raw["run_id"].is_null() || raw["run_id"].as_str() == Some(&run_id);
                    let workspace_matches = raw["workspace_id"].is_null()
                        || raw["workspace_id"].as_str() == Some(&workspace_id);
                    if safe_component(&run_id)
                        && safe_component(&workspace_id)
                        && hosts_match
                        && run_matches
                        && workspace_matches
                    {
                        status = "verified".into();
                        event = raw;
                    } else {
                        status = "malformed".into();
                    }
                } else {
                    status = "malformed".into();
                }
            }
            value["prologue"] = json!({"status": status, "flow_id": id, "host_id": "local", "path": path, "run_start": event});
        }
        sections.get_mut("leads").unwrap().push(envelope(
            format!("lead:{round}"),
            "lead_directory",
            value,
        ));
    }
    let mut board_records = 0usize;
    let mut board_key_bytes = 0usize;
    for relative in [
        "board/claims.jsonl",
        "board/directives.jsonl",
        "board/posts.jsonl",
    ] {
        let rows = input.jsonl(relative)?;
        if !relative.ends_with("posts.jsonl") {
            board_records += rows.len();
            board_key_bytes += rows
                .iter()
                .map(|r| r["value"]["key"].as_str().map_or(64, |s| s.len() + 64))
                .sum::<usize>();
        }
        if relative.ends_with("posts.jsonl") {
            for row in rows {
                sections.get_mut("board").unwrap().push(envelope(
                    format!("post:{}", row["line"]),
                    "board_post",
                    row["value"].clone(),
                ));
            }
        }
    }
    for entry in input.entries(Path::new("board/claims"))? {
        if entry.is_file
            && Path::new(&entry.name)
                .extension()
                .is_some_and(|s| s == "json")
        {
            if let Some(bytes) =
                input.copy(&PathBuf::from("board/claims").join(entry.name), MAX_LINE)?
            {
                board_records += 1;
                let value = serde_json::from_slice::<Value>(&bytes).unwrap_or(Value::Null);
                board_key_bytes += value["key"]
                    .as_str()
                    .map_or(bytes.len() + 64, |s| s.len() + 64);
            }
        }
    }
    // The legacy claim fold reconciles live files against journal entries.
    // Bound that work before invoking it, not after materializing its output.
    if board_records > 4_000 || board_records.saturating_mul(board_key_bytes) > 256 * 1024 * 1024 {
        return Err(too_large("board projection exceeds the finite work budget"));
    }
    let now = Utc::now();
    let board = rupu_runtime::services::board::board_view(input.mirror.path(), now)
        .map_err(|_| ApiError::conflict("captured board cannot be projected"))?;
    for (index, claim) in board.claims.into_iter().enumerate() {
        sections.get_mut("board").unwrap().push(envelope(
            format!("claim:{index}"),
            "board_claim",
            serde_json::to_value(claim)
                .map_err(|_| ApiError::internal("invalid captured claim"))?,
        ));
    }
    for (index, directive) in board.directives.into_iter().enumerate() {
        sections.get_mut("board").unwrap().push(envelope(
            format!("directive:{index}:{}", directive.id),
            "board_directive",
            serde_json::to_value(directive)
                .map_err(|_| ApiError::internal("invalid captured directive"))?,
        ));
    }
    let token = ulid::Ulid::new().to_string();
    let mut key = [0_u8; 32];
    key[..16].copy_from_slice(&ulid::Ulid::new().to_bytes());
    key[16..].copy_from_slice(&ulid::Ulid::new().to_bytes());
    let counts: BTreeMap<_, _> = sections
        .iter()
        .map(|(name, rows)| (name, json!({"total": rows.len()})))
        .collect();
    let summary = json!({
        "flow_id": id, "host_id": "local", "snapshot": token,
        "source_generation": format!("{:x}", input.hash.finalize()),
        "captured_at": now, "expires_at": now + chrono::Duration::seconds(TTL.as_secs() as i64),
        "record": record_dto(record), "definition": definition, "budget_state": budget,
        "latest_lead": sections["leads"].last().map(|r| &r["value"]),
        "sections": counts, "board_journal": board.journal,
        "evidence": {"consistency": "read_time_observation", "malformed_records": input.malformed,
            "incomplete_lines": input.partial, "input_bytes": input.bytes,
            "board_projection": "legacy_tolerant", "definition_status": definition_status,
            "project_scope": "verify_from_lead_transcript", "latest_lead_prologue": "bounded_first_record",
            "events": "coordinator_events_only", "unit_outputs": "fetch_on_demand"},
    });
    if serde_json::to_vec(&summary)
        .map_err(|_| ApiError::internal("invalid capture summary"))?
        .len()
        > MAX_PAGE
    {
        return Err(too_large("capture summary exceeds response budget"));
    }
    let bytes = serde_json::to_vec(&sections)
        .map_err(|_| ApiError::internal("invalid capture"))?
        .len();
    if bytes > MAX_CAPTURE || sections.values().map(Vec::len).sum::<usize>() > MAX_ROWS {
        return Err(too_large(
            "captured observation exceeds retained-data budget",
        ));
    }
    Ok(Captured {
        id: id.into(),
        token,
        key,
        born: Instant::now(),
        summary,
        sections,
        bytes,
    })
}
