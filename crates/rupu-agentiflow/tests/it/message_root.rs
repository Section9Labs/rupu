//! The operator's view of an agentiflow (spec F1b §3.1): `MessageRoot`
//! reads a real `AgentiflowRecord` as the flow writes it, live only while
//! it is running under a live coordinator, and addresses the lead by
//! default.

use rupu_agentiflow::{agentiflow_dir, pid_is_running};
use rupu_fleet::Address;
use rupu_runtime::services::operator::{Liveness, MessageRoot, RootKind};
use tempfile::tempdir;

use crate::reaper::{a_dead_pid, write_record};

#[test]
fn agentiflow_record_projection_reads_live_and_not_live() {
    let global = tempdir().unwrap();
    let g = global.path();
    write_record(&global, "af_live", "running", Some(std::process::id()));
    let root = MessageRoot::resolve(g, "af_live").unwrap();
    assert_eq!(root.kind, RootKind::Agentiflow);
    assert_eq!(root.space, agentiflow_dir(g).join("af_live"));
    assert_eq!(root.default_recipient(), Address::Lead);
    assert_eq!(root.liveness(g, &pid_is_running), Liveness::Live);

    write_record(&global, "af_live", "running", None);
    assert_eq!(
        root.liveness(g, &pid_is_running),
        Liveness::Live,
        "a record naming no coordinator"
    );

    let dead = a_dead_pid();
    write_record(&global, "af_live", "running", Some(dead));
    assert_eq!(
        root.liveness(g, &pid_is_running),
        Liveness::NotLive(format!(
            "agentiflow `af_live`'s coordinator (pid {dead}) is no longer running"
        ))
    );

    for status in ["completed", "stopped", "interrupted", "failed"] {
        write_record(&global, "af_live", status, Some(std::process::id()));
        assert_eq!(
            root.liveness(g, &pid_is_running),
            Liveness::NotLive(format!("agentiflow `af_live` is not running ({status})")),
            "{status}"
        );
    }
}
