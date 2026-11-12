//! Display provenance from already captured typed records. Never file membership.
use super::*;
use rupu_orchestrator::executor::Event;
const MAX_REFERENCES: usize = 32;
#[derive(Default)]
struct References {
    ids: Vec<String>,
    truncated: bool,
}
#[derive(Default)]
pub(super) struct Index {
    entries: BTreeMap<String, References>,
    pub bytes: usize,
}
pub(super) enum Classified {
    Root,
    Nested(Value),
    Unverified(Value),
    Malformed,
}
fn identifier(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 128
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}
impl Index {
    pub(super) fn build(
        sections: &BTreeMap<String, Vec<Vec<u8>>>,
        input: &input::Input,
        captured_bytes: usize,
    ) -> ApiResult<Self> {
        let mut index = Self::default();
        for (section, rows) in sections {
            let field = match section.as_str() {
                "results" | "checkpoints" => "run_id",
                "attempts" => "agent_run_id",
                _ if section.starts_with("items/result:") => "run_id",
                _ => continue,
            };
            for bytes in rows {
                input.deadline()?;
                // These rows were emitted only after their owning source passed
                // StepResultRecord / UnitCheckpoint / AttemptRecord validation.
                let row: Value = serde_json::from_slice(bytes).expect("captured row");
                if row["state"] != "available" {
                    continue;
                }
                let Some(run) = row["value"][field].as_str().filter(|s| identifier(s)) else {
                    continue;
                };
                let Some(id) = row["record_id"].as_str() else {
                    continue;
                };
                index.add(run, id);
                if index.bytes + captured_bytes > MAX_CAPTURE {
                    return Err(too_large("captured event reference index limit"));
                }
            }
        }
        input.deadline()?;
        Ok(index)
    }
    fn add(&mut self, run: &str, id: &str) {
        if !self.entries.contains_key(run) {
            self.bytes += run.len() + std::mem::size_of::<References>();
        }
        let refs = self.entries.entry(run.into()).or_default();
        if refs.ids.iter().any(|v| v == id) {
            return;
        }
        if refs.ids.len() == MAX_REFERENCES {
            refs.truncated = true;
            return;
        }
        refs.ids.push(id.into());
        self.bytes += id.len() + std::mem::size_of::<String>();
    }
    pub(super) fn classify(&self, value: &Value, owner: &str) -> Classified {
        let event = match serde_json::from_value::<Event>(value.clone()) {
            Ok(e) => e,
            Err(_) => return Classified::Malformed,
        };
        if !matches!(event, Event::Unknown) && event.run_id() == owner {
            return Classified::Root;
        }
        if matches!(
            event,
            Event::DispatchStarted { .. } | Event::DispatchCompleted { .. }
        ) {
            if let Some(refs) = self.entries.get(event.run_id()) {
                return Classified::Nested(
                    json!({"scope":"captured_workflow_agent","run_id":event.run_id(),"reference_record_ids":refs.ids,"references_truncated":refs.truncated}),
                );
            }
        }
        let run = value["run_id"].as_str().filter(|s| identifier(s));
        Classified::Unverified(
            json!({"scope":"unverified","run_id":run,"reference_record_ids":[],"references_truncated":false}),
        )
    }
}
pub(super) fn diagnostic_type(value: &Value) -> Option<&str> {
    value["type"]
        .as_str()
        .filter(|v| !v.is_empty() && v.len() <= 128)
}
#[cfg(test)]
mod tests;
