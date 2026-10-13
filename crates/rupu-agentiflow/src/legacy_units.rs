//! The units an agentiflow launched before W7 (`units/<id>/unit.json`, which
//! the old fleet supervisor mirrored each unit to). New runs record their
//! children in `children.jsonl` (`rupu_launch::ledger`); this reader keeps
//! older runs readable for the CP units view and the orphan reaper (D20: no
//! on-disk migration).

use std::path::Path;

/// What kind of thing a unit runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum UnitKind {
    /// One agent (`rupu run <agent>`).
    #[default]
    Agent,
    /// One workflow (`rupu workflow run <name>`).
    Workflow,
}

impl UnitKind {
    /// The stable lowercase name (`unit.json`'s `kind`).
    pub fn as_str(self) -> &'static str {
        match self {
            UnitKind::Agent => "agent",
            UnitKind::Workflow => "workflow",
        }
    }
}

/// A unit's final answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnitOutcome {
    pub output: String,
    pub success: bool,
}

/// Where a unit is in its life.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UnitStatus {
    /// Spawned, no evidence yet that it has started.
    Pending,
    /// Started and alive.
    Running,
    /// Ended with an answer (which may itself report `success: false`).
    Done(UnitOutcome),
    /// Ended without an answer (crashed, killed, vanished); carries why.
    Failed(String),
}

impl UnitStatus {
    /// Whether this status is final (no further transitions).
    pub fn is_terminal(&self) -> bool {
        matches!(self, UnitStatus::Done(_) | UnitStatus::Failed(_))
    }
}

/// Whether `id` is a plain run id, safe to use as a single path component.
fn is_safe_unit_id(id: &str) -> bool {
    !id.is_empty()
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// One unit as `units/<id>/unit.json` records it, read back by
/// [`units_on_disk`]: the facts a process that did not launch the unit (the
/// orphan reaper, `agentiflow stop`) needs to wind it down.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnitOnDisk {
    /// The unit's run id (the `units/<id>` directory name).
    pub unit_id: String,
    /// The process group the unit leads. `None` for a `unit.json` written
    /// before the pgid was recorded, or by a launcher with no process.
    pub pgid: Option<u32>,
    pub kind: UnitKind,
    /// The last status the supervisor mirrored to disk.
    pub status: UnitStatus,
}

/// What `unit.json` holds, parsed tolerantly: only what [`UnitOnDisk`] needs,
/// with every key but the status optional so an older file still reads.
#[derive(serde::Deserialize)]
struct UnitJson {
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    pgid: Option<u32>,
    status: StatusJson,
}

/// The `status` object [`status_json`] writes.
#[derive(serde::Deserialize)]
struct StatusJson {
    state: String,
    #[serde(default)]
    success: Option<bool>,
    #[serde(default)]
    output: Option<String>,
    #[serde(default)]
    error: Option<String>,
}

impl StatusJson {
    /// `None` for a state this build does not know (a bad file, not a guess).
    fn into_status(self) -> Option<UnitStatus> {
        Some(match self.state.as_str() {
            "pending" => UnitStatus::Pending,
            "running" => UnitStatus::Running,
            "done" => UnitStatus::Done(UnitOutcome {
                output: self.output.unwrap_or_default(),
                success: self.success.unwrap_or(false),
            }),
            "failed" => UnitStatus::Failed(self.error.unwrap_or_default()),
            _ => return None,
        })
    }
}

fn parse_unit_json(path: &Path, unit_id: &str) -> Option<UnitOnDisk> {
    let raw = match std::fs::read(path) {
        Ok(raw) => raw,
        // A unit dir whose unit.json was never written is not an error worth a
        // log line (the write is tmp + rename, so it is never half there).
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return None,
        Err(e) => {
            tracing::warn!(unit = unit_id, error = %e, "could not read unit.json; skipping");
            return None;
        }
    };
    let parsed: UnitJson = match serde_json::from_slice(&raw) {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!(unit = unit_id, error = %e, "malformed unit.json; skipping");
            return None;
        }
    };
    let kind = match parsed.kind.as_deref() {
        None | Some("agent") => UnitKind::Agent,
        Some("workflow") => UnitKind::Workflow,
        Some(other) => {
            tracing::warn!(
                unit = unit_id,
                kind = other,
                "unknown unit kind in unit.json; skipping"
            );
            return None;
        }
    };
    let Some(status) = parsed.status.into_status() else {
        tracing::warn!(unit = unit_id, "unknown unit state in unit.json; skipping");
        return None;
    };
    Some(UnitOnDisk {
        unit_id: unit_id.to_string(),
        pgid: parsed.pgid,
        kind,
        status,
    })
}

/// Every unit `run_dir` records, oldest first (ids are ULIDs), read from
/// `units/*/unit.json`.
///
/// This is what lets a process other than the supervisor find a run's units:
/// the supervisor's in-memory map dies with its process, `unit.json` does not.
/// Reading is tolerant by design (a reaper sweep must survive any run dir): a
/// missing `units/`, a unit dir without a `unit.json`, a torn or unparseable
/// file, or an unknown kind / state is skipped (the unparseable ones with a
/// warning), never a panic or an error.
pub fn units_on_disk(run_dir: &Path) -> Vec<UnitOnDisk> {
    let units_dir = run_dir.join("units");
    let entries = match std::fs::read_dir(&units_dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Vec::new(),
        Err(e) => {
            tracing::warn!(dir = %units_dir.display(), error = %e, "could not list units; skipping");
            return Vec::new();
        }
    };
    let mut out: Vec<UnitOnDisk> = entries
        .filter_map(|entry| {
            let entry = entry.ok()?;
            // A stray file where a unit dir is expected has no unit.json under it.
            let unit_id = entry.file_name().into_string().ok()?;
            // `write_unit_json` only ever writes plain run ids; the id is handed
            // on as a path component, so anything else is not ours.
            if !is_safe_unit_id(&unit_id) {
                tracing::warn!(unit = %unit_id, "unit dir is not a plain run id; skipping");
                return None;
            }
            parse_unit_json(&entry.path().join("unit.json"), &unit_id)
        })
        .collect();
    out.sort_by(|a, b| a.unit_id.cmp(&b.unit_id));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn units_on_disk_reads_a_unit_json_written_before_pgid_existed() {
        let dir = tempfile::tempdir().unwrap();
        let unit = dir.path().join("units").join("run_OLD");
        std::fs::create_dir_all(&unit).unwrap();
        // Exactly the pre-4-3 shape: no `pgid` key at all.
        std::fs::write(
            unit.join("unit.json"),
            r#"{"run_id":"run_OLD","agent":"web-assess","kind":"workflow",
               "participant":"web-assess#1","started_at":"2026-10-06T00:00:00+00:00",
               "status":{"state":"done","success":true,"output":"ok"}}"#,
        )
        .unwrap();
        let units = units_on_disk(dir.path());
        assert_eq!(units.len(), 1, "{units:?}");
        assert_eq!(units[0].unit_id, "run_OLD");
        assert_eq!(units[0].pgid, None);
        assert_eq!(units[0].kind, UnitKind::Workflow);
        assert_eq!(
            units[0].status,
            UnitStatus::Done(UnitOutcome {
                output: "ok".into(),
                success: true
            })
        );
    }

    #[test]
    fn units_on_disk_skips_a_bad_unit_json_and_never_panics() {
        let dir = tempfile::tempdir().unwrap();
        // No `units/` dir at all: nothing, not an error.
        assert!(units_on_disk(dir.path()).is_empty());

        let units = dir.path().join("units");
        let write = |name: &str, body: &str| {
            let d = units.join(name);
            std::fs::create_dir_all(&d).unwrap();
            std::fs::write(d.join("unit.json"), body).unwrap();
        };
        write("run_a_torn", "{ not json");
        write(
            "run_b_unknown_state",
            r#"{"kind":"agent","status":{"state":"levitating"}}"#,
        );
        write(
            "run_c_unknown_kind",
            r#"{"kind":"sorcery","status":{"state":"running"}}"#,
        );
        write(
            "run_d_pgid_out_of_range",
            r#"{"kind":"agent","pgid":99999999999,"status":{"state":"running"}}"#,
        );
        write(
            "not a run id",
            r#"{"kind":"agent","pgid":88,"status":{"state":"running"}}"#,
        );
        // A dir with no unit.json, and a stray file where a dir is expected.
        std::fs::create_dir_all(units.join("run_e_empty")).unwrap();
        std::fs::write(units.join("stray.txt"), "x").unwrap();
        write(
            "run_f_good",
            r#"{"kind":"agent","pgid":77,"status":{"state":"failed","error":"boom"}}"#,
        );

        let got = units_on_disk(dir.path());
        let ids: Vec<&str> = got.iter().map(|u| u.unit_id.as_str()).collect();
        assert_eq!(ids, ["run_f_good"], "{got:?}");
        assert_eq!(got[0].pgid, Some(77));
        assert_eq!(got[0].status, UnitStatus::Failed("boom".into()));
    }
}
