use super::input::Input;
use super::*;
use rupu_orchestrator::{
    runs::{AttemptRecord, UnitCheckpoint},
    StepResultRecord, Workflow,
};
use sha2::Digest;

const MAX_LINE: usize = 8 * 1024 * 1024;
const MAX_ROWS: usize = 100_000;
const BASE_SECTIONS: [&str; 6] = [
    "steps",
    "edges",
    "results",
    "checkpoints",
    "attempts",
    "events",
];

struct Builder {
    sections: BTreeMap<String, Vec<Vec<u8>>>,
    result_steps: BTreeMap<String, Vec<usize>>,
    result_records: BTreeMap<String, usize>,
    bytes: usize,
    rows: usize,
    unavailable: usize,
    emitters: Option<event_emitters::Index>,
}
impl Builder {
    fn new() -> Self {
        Self {
            sections: BASE_SECTIONS
                .into_iter()
                .map(|s| (s.into(), Vec::new()))
                .collect(),
            result_steps: BTreeMap::new(),
            result_records: BTreeMap::new(),
            bytes: 0,
            rows: 0,
            unavailable: 0,
            emitters: None,
        }
    }
    fn push(&mut self, section: &str, mut row: Value) -> ApiResult<()> {
        let mut bytes =
            serde_json::to_vec(&row).map_err(|_| ApiError::internal("record encoding failed"))?;
        if bytes.len() > MAX_ROW {
            // Never truncate an authority field or present a clipped object as complete.
            row = json!({"record_id":row["record_id"],"byte_offset":row["byte_offset"],"parent_record_id":row["parent_record_id"],"index":row["index"],"state":"oversized","value":null});
            bytes = serde_json::to_vec(&row).expect("placeholder JSON");
            self.unavailable += 1;
        }
        self.rows += 1;
        self.bytes += bytes.len();
        if self.rows > MAX_ROWS
            || self.bytes + self.emitters.as_ref().map_or(0, |v| v.bytes) > MAX_CAPTURE
        {
            return Err(too_large("workflow captured row/byte limit exceeded"));
        }
        self.sections.entry(section.into()).or_default().push(bytes);
        Ok(())
    }
    fn row(
        &mut self,
        section: &str,
        kind: &str,
        offset: usize,
        state: &str,
        value: Value,
    ) -> ApiResult<()> {
        self.push(section,json!({"record_id":format!("{kind}:{offset}"),"byte_offset":offset.to_string(),"state":state,"value":value}))
    }
    fn result(&mut self, offset: usize, value: Value) -> ApiResult<()> {
        let id = format!("result:{offset}");
        let mut object = value.as_object().expect("validated result object").clone();
        let items = object
            .remove("items")
            .and_then(|v| v.as_array().cloned())
            .unwrap_or_default();
        let findings = object
            .remove("findings")
            .and_then(|v| v.as_array().cloned())
            .unwrap_or_default();
        let mut header = json!({"record_id":id,"byte_offset":offset.to_string(),"state":"available","items_count":items.len(),"findings_count":findings.len(),"omitted_fields":["items","findings"],"payload_state":"included","children_complete":true,"value":object});
        if serde_json::to_vec(&header).expect("result JSON").len() > MAX_ROW {
            // Fan-out output often repeats every child output. Preserve bounded
            // identity/status and child membership, explicitly omitting only
            // these payload fields. Never clip an identity or replace defaults.
            for field in ["output", "rendered_prompt", "run_outcome"] {
                if header["value"]
                    .as_object_mut()
                    .expect("result object")
                    .remove(field)
                    .is_some()
                {
                    header["omitted_fields"]
                        .as_array_mut()
                        .expect("omitted array")
                        .push(json!(field));
                }
            }
            header["payload_state"] = json!("omitted_oversized");
        }
        let available = serde_json::to_vec(&header).expect("result JSON").len() <= MAX_ROW;
        if available {
            self.result_records
                .insert(id.clone(), self.sections["results"].len());
            if let Some(step) = header["value"]["step_id"]
                .as_str()
                .filter(|s| !s.is_empty() && s.len() <= 1024)
            {
                self.result_steps
                    .entry(step.to_owned())
                    .or_default()
                    .push(self.sections["results"].len());
            }
        }
        self.push("results", header)?;
        if !available {
            return Ok(());
        }
        for (section, children) in [("items", &items), ("findings", &findings)] {
            let key = format!("{section}/{id}");
            self.sections.entry(key.clone()).or_default();
            for (index, item) in children.iter().enumerate() {
                self.push(&key,json!({"record_id":format!("{id}/{section}:{index}"),"parent_record_id":id,"index":index,"state":"available","value":item}))?;
            }
        }
        Ok(())
    }
    fn log(
        &mut self,
        input: &mut Input,
        name: &str,
        section: &str,
        kind: &str,
        owner: &str,
    ) -> ApiResult<Value> {
        let enriched = section == "events" && self.emitters.is_some();
        let Some(read) = input.read(name, input::MAX_FILE, true)? else {
            let mut evidence = json!({"state":"missing","complete":false,"captured_rows":0,"source_total":null,"malformed_rows":0,"partial_rows":0,"oversized_rows":0});
            if enriched {
                evidence["nested_emitter_rows"] = json!(0);
                evidence["unverified_emitter_rows"] = json!(0);
            }
            return Ok(evidence);
        };
        let base = usize::try_from(read.start_offset)
            .map_err(|_| too_large("workflow byte offset exceeds host range"))?;
        let mut offset = 0;
        let mut malformed = 0;
        let mut nested_emitters = 0;
        let mut unverified_emitters = 0;
        let mut partial = 0;
        let mut oversized = 0;
        let before = self.sections[section].len();
        while offset < read.bytes.len() {
            input.deadline()?;
            let rest = &read.bytes[offset..];
            let len = rest
                .iter()
                .position(|b| *b == b'\n')
                .map(|n| n + 1)
                .unwrap_or(rest.len());
            let line = &rest[..len];
            let state = if !line.ends_with(b"\n") {
                partial += 1;
                "partial"
            } else if line.len() > MAX_LINE {
                oversized += 1;
                "oversized"
            } else {
                "available"
            };
            if state != "available" {
                self.row(section, kind, base + offset, state, Value::Null)?;
            } else if !line.iter().all(u8::is_ascii_whitespace) {
                let value = serde_json::from_slice::<Value>(line).ok();
                if enriched {
                    use event_emitters::Classified;
                    let classified = value.as_ref().map_or(Classified::Malformed, |v| {
                        self.emitters
                            .as_ref()
                            .expect("enriched index")
                            .classify(v, owner)
                    });
                    match classified {
                        Classified::Root => {
                            self.row(section, kind, base + offset, "available", value.unwrap())?
                        }
                        Classified::Nested(emitter) => {
                            nested_emitters += 1;
                            self.push(section,json!({"record_id":format!("{kind}:{}",base+offset),"byte_offset":(base+offset).to_string(),"state":"available","value":value.unwrap(),"emitter":emitter}))?;
                        }
                        Classified::Unverified(emitter) => {
                            unverified_emitters += 1;
                            self.unavailable += 1;
                            let event_type =
                                value.as_ref().and_then(event_emitters::diagnostic_type);
                            self.push(section,json!({"record_id":format!("{kind}:{}",base+offset),"byte_offset":(base+offset).to_string(),"state":"unavailable","reason":"event_emitter_unverified","value":null,"event_type":event_type,"emitter":emitter}))?;
                        }
                        Classified::Malformed => {
                            malformed += 1;
                            self.row(section, kind, base + offset, "malformed", Value::Null)?;
                        }
                    }
                    offset += len;
                    continue;
                }
                let valid = value.as_ref().is_some_and(|v| match section {
                    "results" => serde_json::from_value::<StepResultRecord>(v.clone()).is_ok(),
                    "checkpoints" => serde_json::from_value::<UnitCheckpoint>(v.clone()).is_ok(),
                    "attempts" => {
                        serde_json::from_value::<AttemptRecord>(v.clone()).is_ok_and(|r| r.v == 1)
                    }
                    "events" => {
                        serde_json::from_value::<rupu_orchestrator::executor::Event>(v.clone())
                            .is_ok_and(|e| e.run_id() == owner)
                    }
                    _ => false,
                });
                if valid {
                    let value = value.unwrap();
                    if section == "results" {
                        self.result(base + offset, value)?;
                    } else {
                        self.row(section, kind, base + offset, "available", value)?;
                    }
                } else {
                    malformed += 1;
                    self.row(section, kind, base + offset, "malformed", Value::Null)?;
                }
            }
            offset += len;
        }
        let count = self.sections[section].len() - before;
        let mut evidence = json!({"state":"present","complete":read.complete && partial == 0,"captured_rows":count,
            "source_total":if read.complete && partial == 0 { Some(count) } else { None },
            "start_offset":read.start_offset.to_string(),"head_truncated":read.head_truncated,"leading_skipped_bytes":read.leading_skipped_bytes,"read_bytes":read.read_bytes,"file_bytes_at_read":read.source_bytes,"prefix_truncated":input.window==Window::Prefix && !read.complete,"tail_truncated":read.start_offset+(read.bytes.len() as u64)<read.source_bytes,
            "malformed_rows":malformed,"partial_rows":partial,"oversized_rows":oversized});
        if enriched {
            evidence["nested_emitter_rows"] = json!(nested_emitters);
            evidence["unverified_emitter_rows"] = json!(unverified_emitters);
        }
        Ok(evidence)
    }
}
#[cfg(test)]
pub(super) fn capture(s: &AppState, id: &str, ws: &str) -> ApiResult<Captured> {
    capture_window(s, id, ws, Window::Prefix)
}
#[cfg(test)]
pub(super) fn capture_window(
    s: &AppState,
    id: &str,
    ws: &str,
    window: Window,
) -> ApiResult<Captured> {
    capture_mode(s, id, ws, window, false)
}
pub(super) fn capture_mode(
    s: &AppState,
    id: &str,
    ws: &str,
    window: Window,
    include_event_emitters: bool,
) -> ApiResult<Captured> {
    let mut opened = super::owner::open(s, id, ws)?;
    let owner_bytes = opened.owner_bytes.clone();
    let archived = opened.archived;
    let store_kind = opened.store_kind;
    let input = &mut opened.input;
    input.window = window;
    let mut builder = Builder::new();
    let mut evidence = serde_json::Map::new();
    let source = input.read("workflow.yaml", 1024 * 1024, false)?;
    let source_info = match source {
        None => json!({"state":"missing","sha256":null}),
        Some(read) => {
            let hash = format!("{:x}", Sha256::digest(&read.bytes));
            match std::str::from_utf8(&read.bytes).ok().map(Workflow::parse) {
                Some(Err(rupu_orchestrator::WorkflowParseError::Empty)) => {
                    json!({"state":"available","sha256":hash})
                }
                None | Some(Err(_)) => json!({"state":"malformed","sha256":hash}),
                Some(Ok(wf)) => {
                    if wf.steps.len() > 4096 {
                        return Err(too_large("declared workflow step limit exceeded"));
                    }
                    let dag = super::super::graph::to_step_dag(&wf);
                    if dag.edges.len() > 65536 {
                        return Err(too_large("declared workflow edge limit exceeded"));
                    }
                    for (index, step) in dag.steps.iter().enumerate() {
                        builder.push("steps",json!({"record_id":format!("step:{index}"),"index":index,"state":"available","value":step}))?;
                    }
                    for (index, edge) in dag.edges.iter().enumerate() {
                        builder.push("edges",json!({"record_id":format!("edge:{index}"),"index":index,"state":"available","value":edge}))?;
                    }
                    json!({"state":"available","sha256":hash})
                }
            }
        }
    };
    for section in ["steps", "edges"] {
        let available = source_info["state"] == "available";
        evidence.insert(section.into(),json!({"state":source_info["state"],"captured_rows":builder.sections[section].len(),"complete":available,"source_total":if available {Some(builder.sections[section].len())}else{None}}));
    }
    for (name, section, kind) in [
        ("step_results.jsonl", "results", "result"),
        ("unit_checkpoints.jsonl", "checkpoints", "checkpoint"),
        ("attempts.jsonl", "attempts", "attempt"),
        ("events.jsonl", "events", "event"),
    ] {
        if section == "events" && include_event_emitters {
            builder.emitters = Some(event_emitters::Index::build(
                &builder.sections,
                input,
                builder.bytes,
            )?);
        }
        evidence.insert(section.into(), builder.log(input, name, section, kind, id)?);
    }
    // Classification is finished. Release its transient reference index before
    // the final header/result-index preparation accounting begins.
    builder.emitters = None;
    let indexed = builder.result_steps.values().map(Vec::len).sum::<usize>();
    evidence.get_mut("results").expect("result evidence")["unassigned_rows"] =
        json!(builder.sections["results"].len() - indexed);
    let index_bytes = builder
        .result_steps
        .iter()
        .map(|(key, rows)| key.len() + rows.len() * std::mem::size_of::<usize>())
        .sum::<usize>()
        + builder
            .result_records
            .keys()
            .map(|key| key.len() + std::mem::size_of::<usize>())
            .sum::<usize>();
    let execution = executing::Observation::read(&opened)?;
    opened.verify(id, ws)?;
    let input = &mut opened.input;
    execution.bind(input);
    input.deadline()?;
    let observed = chrono::Utc::now();
    let token = ulid::Ulid::new().to_string();
    let mut key = [0u8; 32];
    key[..16].copy_from_slice(&ulid::Ulid::new().to_bytes());
    key[16..].copy_from_slice(&ulid::Ulid::new().to_bytes());
    let frontier = input.frontier(id, ws, store_kind, &s.workflow_observations.epoch);
    let generation = format!("sha256:{:x}", input.hash.clone().finalize());
    let mut header = json!({"schema":FEATURE,"run_id":id,"workspace_id":ws,"host_id":"local","snapshot":token,"window":window,"frontier":frontier,"source_generation":generation,
        "observed_at":observed,"expires_at":observed+chrono::Duration::seconds(600),"freshness":"finite_read_time_capture","archived":archived,"store_kind":store_kind,
        "run":serde_json::from_slice::<Value>(&owner_bytes).expect("validated owner JSON"),"workflow_source":source_info,"sections":evidence,
        "unavailable_rows":builder.unavailable,"captured_rows":builder.rows,"input_bytes":input.bytes,
        "executing_steps":execution.value,"execution_input_bytes":execution.input_bytes,
        "limits":{"page_bytes":MAX_PAGE,"row_bytes":MAX_ROW,"file_bytes":input::MAX_FILE,"input_bytes":input::MAX_INPUT,"capture_bytes":MAX_CAPTURE,"capture_rows":MAX_ROWS},
        "membership":"recorded_evidence_only; transcript_prologue_verification_required","session_linkage":"not_inferred"});
    if include_event_emitters {
        header["event_emitters"] = json!(true);
    }
    let header_bytes = serde_json::to_vec(&header).expect("header JSON").len();
    if header_bytes > MAX_PAGE || builder.bytes + header_bytes + index_bytes > MAX_CAPTURE {
        return Err(too_large(
            "workflow observation header/capture limit exceeded",
        ));
    }
    Ok(Captured {
        run_id: id.into(),
        workspace: ws.into(),
        token,
        key,
        born: Instant::now(),
        window,
        event_emitters: include_event_emitters,
        header,
        sections: builder.sections,
        result_steps: builder.result_steps,
        result_records: builder.result_records,
        bytes: builder.bytes + header_bytes + index_bytes,
    })
}
