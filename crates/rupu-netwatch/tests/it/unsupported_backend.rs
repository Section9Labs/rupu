use rupu_netflow::{CallAttribution, CaptureState, MemorySink, SubprocessCapture};
use rupu_netwatch::unsupported::UnsupportedCapture;
use std::sync::Arc;

fn attribution(run: &str, call: &str, sink: &Arc<MemorySink>) -> CallAttribution {
    CallAttribution {
        run_id: run.into(),
        step_id: None,
        agent: None,
        codename: None,
        tool_call_id: call.into(),
        sink: sink.clone(),
    }
}

fn backend() -> UnsupportedCapture {
    UnsupportedCapture::new("cgroup v2 not mounted")
}

/// Let spawned one-shot writes run to completion.
async fn settle() {
    for _ in 0..5 {
        tokio::task::yield_now().await;
    }
}

#[tokio::test]
async fn announces_unavailable_once_per_run() {
    let cap = backend();
    let sink1 = Arc::new(MemorySink::default());
    let sink2 = Arc::new(MemorySink::default());

    let c1 = cap.begin(attribution("run-1", "toolu_a", &sink1));
    let c2 = cap.begin(attribution("run-1", "toolu_b", &sink1));
    let c3 = cap.begin(attribution("run-2", "toolu_c", &sink2));
    settle().await;

    assert!(c1.shell_prefix().is_none());
    assert!(c2.shell_prefix().is_none());
    assert!(c3.shell_prefix().is_none());

    let want = CaptureState::Unavailable {
        reason: "cgroup v2 not mounted".into(),
    };
    let l1 = sink1.capture_states();
    assert_eq!(l1.len(), 1);
    assert_eq!(l1[0].state, want);
    assert_eq!(l1[0].tool_call_id.as_deref(), Some("toolu_a"));
    let l2 = sink2.capture_states();
    assert_eq!(l2.len(), 1);
    assert_eq!(l2[0].state, want);
    assert_eq!(l2[0].tool_call_id.as_deref(), Some("toolu_c"));
}

#[tokio::test]
async fn run_finished_allows_reannounce() {
    let cap = backend();
    let sink = Arc::new(MemorySink::default());

    let _ = cap.begin(attribution("run-1", "toolu_a", &sink));
    settle().await;
    assert_eq!(sink.capture_states().len(), 1);

    cap.run_finished("run-1");
    let _ = cap.begin(attribution("run-1", "toolu_b", &sink));
    settle().await;
    assert_eq!(sink.capture_states().len(), 2);
}

#[test]
fn announces_without_a_tokio_runtime() {
    let cap = backend();
    let sink = Arc::new(MemorySink::default());
    let _ = cap.begin(attribution("run-1", "toolu_a", &sink));
    assert_eq!(sink.capture_states().len(), 1);
}
