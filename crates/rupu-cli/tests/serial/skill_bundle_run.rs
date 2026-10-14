//! `rupu run --skill-bundle` runs with the shipped skills standing in for
//! installed ones, records the manifest with the rest of its command, and
//! `--continue` reuses it (spec 2026-10-13 skills-on-placed-units, P2/P4).

use crate::ENV_LOCK;
use assert_cmd::Command;
use predicates::prelude::*;
use rupu_skills::ship::{cache_dir, pack_skill, store, SkillBundleManifest};
use rupu_skills::{load_skill, Layer};
use std::path::{Path, PathBuf};

const MOCK_SCRIPT: &str = r#"
{ "turns": [ { "AssistantText": { "text": "ok", "stop": "end_turn" } } ] }
"#;

/// A home whose only skill is the one shipped: the agent grants `pdf` and
/// there is no `skills/` dir.
fn home_with_agent(dir: &Path) {
    let agents = dir.join(".rupu/agents");
    std::fs::create_dir_all(&agents).unwrap();
    std::fs::write(
        agents.join("a.md"),
        "---\nname: a\nprovider: anthropic\nmodel: claude-sonnet-4-6\nskills: [pdf]\n---\nyou answer.\n",
    )
    .unwrap();
}

/// Pack a `pdf` skill from a scratch dir and store it in the host cache;
/// returns the manifest that names it.
fn ship_pdf(dir: &Path) -> SkillBundleManifest {
    let scratch = tempfile::tempdir().unwrap();
    let root = scratch.path().join("pdf");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(
        root.join("SKILL.md"),
        "---\nname: pdf\ndescription: Read PDFs.\n---\n\nDo the pdf thing.\n",
    )
    .unwrap();
    let packed = pack_skill(&load_skill(&root, Layer::Global).unwrap()).unwrap();
    let r = packed.reference();
    store(&cache_dir(&dir.join(".rupu")), &r, &packed.bytes).unwrap();
    SkillBundleManifest {
        version: SkillBundleManifest::VERSION,
        skills: vec![r],
    }
}

/// Like [`ship_pdf`] with a different body, so a different hash.
fn ship_other_pdf(dir: &Path) -> SkillBundleManifest {
    let scratch = tempfile::tempdir().unwrap();
    let root = scratch.path().join("pdf");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(
        root.join("SKILL.md"),
        "---\nname: pdf\ndescription: Read PDFs.\n---\n\nDo the other pdf thing.\n",
    )
    .unwrap();
    let packed = pack_skill(&load_skill(&root, Layer::Global).unwrap()).unwrap();
    let r = packed.reference();
    store(&cache_dir(&dir.join(".rupu")), &r, &packed.bytes).unwrap();
    SkillBundleManifest {
        version: SkillBundleManifest::VERSION,
        skills: vec![r],
    }
}

fn rupu(dir: &Path) -> Command {
    let mut cmd = Command::cargo_bin("rupu").unwrap();
    cmd.current_dir(dir)
        .env("RUPU_MOCK_PROVIDER_SCRIPT", MOCK_SCRIPT)
        .env("RUPU_HOME", dir.join(".rupu"))
        .write_stdin("");
    cmd
}

fn transcript(dir: &Path, run_id: &str) -> PathBuf {
    dir.join(".rupu/transcripts")
        .join(format!("{run_id}.jsonl"))
}

/// The manifest the run recorded with its command (`run_start`'s
/// `launch_args`), for `--continue` to restore.
fn recorded_bundle(dir: &Path, run_id: &str) -> SkillBundleManifest {
    let start = std::fs::read_to_string(transcript(dir, run_id))
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str::<serde_json::Value>(l).unwrap())
        .find(|e| e["type"] == "run_start")
        .unwrap();
    let json = start["data"]["launch_args"]
        .as_array()
        .unwrap()
        .iter()
        .find_map(|a| a.as_str().unwrap().strip_prefix("--skill-bundle="))
        .unwrap()
        .to_string();
    serde_json::from_str(&json).unwrap()
}

/// Cut a run's transcript back to before its first turn: an interrupted
/// run, which `--continue` picks up (a finished one only prints its answer).
fn interrupt(dir: &Path, run_id: &str) {
    let prev = transcript(dir, run_id);
    let kept: Vec<String> = std::fs::read_to_string(&prev)
        .unwrap()
        .lines()
        .take_while(|l| !l.contains("\"turn_start\""))
        .map(String::from)
        .collect();
    std::fs::write(&prev, kept.join("\n") + "\n").unwrap();
}

fn shipped_pdf_indexed(dir: &Path, run_id: &str) -> bool {
    std::fs::read_to_string(transcript(dir, run_id))
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str::<serde_json::Value>(l).unwrap())
        .filter(|e| e["type"] == "skill_index")
        .flat_map(|e| e["data"]["skills"].as_array().cloned().unwrap_or_default())
        .any(|s| s["name"] == "pdf" && s["shipped"] == true)
}

#[tokio::test(flavor = "multi_thread")]
async fn a_shipped_skill_runs_is_recorded_and_continues() {
    let _guard = ENV_LOCK.lock().await;
    let dir = tempfile::tempdir().unwrap();
    home_with_agent(dir.path());
    let manifest = ship_pdf(dir.path());
    let json = serde_json::to_string(&manifest).unwrap();

    rupu(dir.path())
        .args([
            "run",
            "a",
            "--mode",
            "bypass",
            "--run-id",
            "run_shipped",
            &format!("--skill-bundle={json}"),
            "--prompt=hi",
        ])
        .assert()
        .success();
    assert!(shipped_pdf_indexed(dir.path(), "run_shipped"));
    assert_eq!(recorded_bundle(dir.path(), "run_shipped"), manifest);

    // Continue it with no flag: the prior run's bundle carries over.
    interrupt(dir.path(), "run_shipped");
    rupu(dir.path())
        .args([
            "run",
            "a",
            "--mode",
            "bypass",
            "--run-id",
            "run_next",
            "--continue",
            "run_shipped",
        ])
        .assert()
        .success();
    assert!(shipped_pdf_indexed(dir.path(), "run_next"));
    // The continued run records the bundle too, so it can be continued.
    assert_eq!(recorded_bundle(dir.path(), "run_next"), manifest);
}

/// A run recorded before `launch_args` existed kept its bundle in its run
/// directory (`skill_bundle.json`); a continue of it still finds it there.
#[tokio::test(flavor = "multi_thread")]
async fn a_continue_of_a_run_recorded_before_launch_args_reads_its_saved_bundle() {
    let _guard = ENV_LOCK.lock().await;
    let dir = tempfile::tempdir().unwrap();
    home_with_agent(dir.path());
    let manifest = ship_pdf(dir.path());
    let json = serde_json::to_string(&manifest).unwrap();
    rupu(dir.path())
        .args([
            "run",
            "a",
            "--mode",
            "bypass",
            "--run-id",
            "run_old",
            &format!("--skill-bundle={json}"),
            "--prompt=hi",
        ])
        .assert()
        .success();
    interrupt(dir.path(), "run_old");
    // What an older rupu left: no `launch_args`, the manifest on disk.
    let prev = transcript(dir.path(), "run_old");
    let lines: Vec<String> = std::fs::read_to_string(&prev)
        .unwrap()
        .lines()
        .map(|l| {
            let mut v: serde_json::Value = serde_json::from_str(l).unwrap();
            if v["type"] == "run_start" {
                v["data"].as_object_mut().unwrap().remove("launch_args");
            }
            v.to_string()
        })
        .collect();
    std::fs::write(&prev, lines.join("\n") + "\n").unwrap();
    std::fs::create_dir_all(dir.path().join(".rupu/runs/run_old")).unwrap();
    std::fs::write(
        dir.path().join(".rupu/runs/run_old/skill_bundle.json"),
        &json,
    )
    .unwrap();

    rupu(dir.path())
        .args([
            "run",
            "a",
            "--mode",
            "bypass",
            "--run-id",
            "run_after",
            "--continue",
            "run_old",
        ])
        .assert()
        .success();
    assert!(shipped_pdf_indexed(dir.path(), "run_after"));
    assert_eq!(recorded_bundle(dir.path(), "run_after"), manifest);
}

#[tokio::test(flavor = "multi_thread")]
async fn an_explicit_bundle_wins_over_the_continued_runs() {
    let _guard = ENV_LOCK.lock().await;
    let dir = tempfile::tempdir().unwrap();
    home_with_agent(dir.path());
    let manifest = ship_pdf(dir.path());
    let json = serde_json::to_string(&manifest).unwrap();
    rupu(dir.path())
        .args([
            "run",
            "a",
            "--mode",
            "bypass",
            "--run-id",
            "run_one",
            &format!("--skill-bundle={json}"),
            "--prompt=hi",
        ])
        .assert()
        .success();
    interrupt(dir.path(), "run_one");

    // An explicit manifest naming a different (cached) pdf.
    let other = ship_other_pdf(dir.path());
    let other_json = serde_json::to_string(&other).unwrap();
    assert_ne!(other, manifest);
    rupu(dir.path())
        .args([
            "run",
            "a",
            "--mode",
            "bypass",
            "--run-id",
            "run_two",
            "--continue",
            "run_one",
            &format!("--skill-bundle={other_json}"),
        ])
        .assert()
        .success();
    assert_eq!(recorded_bundle(dir.path(), "run_two"), other);
}

#[tokio::test(flavor = "multi_thread")]
async fn an_uncached_hash_fails_naming_the_skill() {
    let _guard = ENV_LOCK.lock().await;
    let dir = tempfile::tempdir().unwrap();
    home_with_agent(dir.path());
    let mut manifest = ship_pdf(dir.path());
    manifest.skills[0].content_hash = "b".repeat(64);
    let json = serde_json::to_string(&manifest).unwrap();
    rupu(dir.path())
        .args([
            "run",
            "a",
            "--mode",
            "bypass",
            &format!("--skill-bundle={json}"),
            "--prompt=hi",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("global:pdf"))
        .stderr(predicate::str::contains("not in this host's skill cache"));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_malformed_bundle_names_the_flag() {
    let _guard = ENV_LOCK.lock().await;
    let dir = tempfile::tempdir().unwrap();
    home_with_agent(dir.path());
    for (bad, detail) in [
        ("not json", "expected"),
        (r#"{"version":2,"skills":[]}"#, "version 2 is not supported"),
    ] {
        rupu(dir.path())
            .args([
                "run",
                "a",
                "--mode",
                "bypass",
                &format!("--skill-bundle={bad}"),
                "--prompt=hi",
            ])
            .assert()
            .failure()
            .stderr(predicate::str::contains("--skill-bundle"))
            .stderr(predicate::str::contains(detail));
    }
}
