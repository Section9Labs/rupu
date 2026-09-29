//! Real-descriptor pressure on `load_agent_admitted`. Lowers this test
//! process's own RLIMIT_NOFILE, so it lives in its own integration binary
//! and runs its scenarios sequentially in ONE test.

use rupu_agent::fd_budget;
use rustix::process::{getrlimit, setrlimit, Resource, Rlimit};
use std::time::{Duration, Instant};

fn set_soft(n: u64) {
    let hard = getrlimit(Resource::Nofile).maximum;
    setrlimit(
        Resource::Nofile,
        Rlimit {
            current: Some(n),
            maximum: hard,
        },
    )
    .unwrap();
}

/// Open descriptors until usage sits at `target` (above the 80% mark).
fn fill_to(target: u64, dir: &std::path::Path) -> Vec<std::fs::File> {
    let mut held = Vec::new();
    while fd_budget::fd_usage().unwrap().0 < target {
        held.push(std::fs::File::open(dir.join("agents/a0.md")).unwrap());
    }
    held
}

async fn load_many(global: &std::path::Path, n: usize) -> Vec<Result<(), String>> {
    let tasks: Vec<_> = (0..n)
        .map(|i| {
            let g = global.to_path_buf();
            tokio::spawn(async move {
                fd_budget::load_agent_admitted(&g, None, &format!("a{}", i % 10))
                    .await
                    .map(|_| ())
                    .map_err(|e| e.to_string())
            })
        })
        .collect();
    let mut out = Vec::new();
    for t in tasks {
        out.push(t.await.unwrap());
    }
    out
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn fan_out_under_real_fd_pressure_grows_then_paces_never_emfiles() {
    let tmp = tempfile::tempdir().unwrap();
    let global = tmp.path();
    std::fs::create_dir_all(global.join("agents")).unwrap();
    for i in 0..10 {
        std::fs::write(
            global.join(format!("agents/a{i}.md")),
            format!("---\nname: a{i}\n---\nprompt {i}\n"),
        )
        .unwrap();
    }

    // Scenario 1 — growth: no ceiling, limit squeezed to 128 and ~95% used.
    // Admission doubles the limit instead of waiting or failing.
    fd_budget::configure(None);
    set_soft(128);
    let held = fill_to(122, global);
    let start = Instant::now();
    let results = load_many(global, 30).await;
    assert!(results.iter().all(|r| r.is_ok()), "{results:?}");
    assert!(
        start.elapsed() < Duration::from_secs(5),
        "growth should not wait"
    );
    assert!(
        getrlimit(Resource::Nofile).current.unwrap() > 128,
        "limit grew"
    );
    drop(held);

    // Scenario 2 — pacing: ceiling pinned at 128 (max_open_files = 128), so
    // growth is refused. Loads must WAIT until descriptors are released,
    // then all succeed — none fail with "Too many open files".
    set_soft(128);
    fd_budget::configure(Some(128));
    let held = fill_to(122, global);
    let release = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(400)).await;
        drop(held);
    });
    let start = Instant::now();
    let results = load_many(global, 30).await;
    release.await.unwrap();
    assert!(results.iter().all(|r| r.is_ok()), "{results:?}");
    assert!(
        start.elapsed() >= Duration::from_millis(350),
        "loads should have been paced until release, took {:?}",
        start.elapsed()
    );
    assert_eq!(
        getrlimit(Resource::Nofile).current,
        Some(128),
        "ceiling honored"
    );
}
