//! FileTailRunSource yields events as the file grows.

use futures_util::StreamExt;
use rupu_orchestrator::executor::{Event, FileTailRunSource};

#[tokio::test]
async fn yields_lines_as_file_grows() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("events.jsonl");
    // Write one event up front
    std::fs::write(
        &path,
        format!(
            "{}\n",
            serde_json::to_string(&Event::RunStarted {
                event_version: 1,
                run_id: "r1".into(),
                workflow_path: dir.path().to_path_buf(),
                started_at: chrono::Utc::now(),
            })
            .unwrap()
        ),
    )
    .unwrap();

    let mut source = FileTailRunSource::open(&path).await.expect("open");
    let first = source.next().await.expect("first event");
    assert!(matches!(first, Event::RunStarted { .. }));

    // Append another line and assert it's yielded
    use std::io::Write;
    let mut f = std::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap();
    writeln!(
        f,
        "{}",
        serde_json::to_string(&Event::RunCompleted {
            run_id: "r1".into(),
            status: rupu_orchestrator::runs::RunStatus::Completed,
            finished_at: chrono::Utc::now(),
        })
        .unwrap()
    )
    .unwrap();
    drop(f);

    let second = tokio::time::timeout(std::time::Duration::from_secs(2), source.next())
        .await
        .expect("timeout")
        .expect("second event");
    assert!(matches!(second, Event::RunCompleted { .. }));
}

#[tokio::test]
async fn waits_for_file_to_be_created() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("events.jsonl");
    // Do not create the file yet
    let mut source = FileTailRunSource::open(&path).await.expect("open");

    tokio::spawn({
        let path = path.clone();
        async move {
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
            std::fs::write(
                &path,
                format!(
                    "{}\n",
                    serde_json::to_string(&Event::RunStarted {
                        event_version: 1,
                        run_id: "rN".into(),
                        workflow_path: path.parent().unwrap().to_path_buf(),
                        started_at: chrono::Utc::now(),
                    })
                    .unwrap()
                ),
            )
            .unwrap();
        }
    });

    let ev = tokio::time::timeout(std::time::Duration::from_secs(3), source.next())
        .await
        .expect("timeout")
        .expect("event");
    assert!(matches!(ev, Event::RunStarted { .. }));
}
