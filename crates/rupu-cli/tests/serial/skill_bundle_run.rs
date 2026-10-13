//! `rupu run --skill-bundle` runs with the shipped skills standing in for
//! installed ones, records the manifest, and `--continue` reuses it (spec
//! 2026-10-13 skills-on-placed-units, P2/P4).

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
    let recorded: SkillBundleManifest = serde_json::from_str(
        &std::fs::read_to_string(dir.path().join(".rupu/runs/run_shipped/skill_bundle.json"))
            .unwrap(),
    )
    .unwrap();
    assert_eq!(recorded, manifest);

    // Continue it with no flag: the prior run's bundle carries over. A
    // finished run only prints its answer, so cut the transcript back to
    // before its first turn.
    let prev = transcript(dir.path(), "run_shipped");
    let kept: Vec<String> = std::fs::read_to_string(&prev)
        .unwrap()
        .lines()
        .take_while(|l| !l.contains("\"turn_start\""))
        .map(String::from)
        .collect();
    std::fs::write(&prev, kept.join("\n") + "\n").unwrap();
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
    let next: SkillBundleManifest = serde_json::from_str(
        &std::fs::read_to_string(dir.path().join(".rupu/runs/run_next/skill_bundle.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(next, manifest);
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
    let prev = transcript(dir.path(), "run_one");
    let kept: Vec<String> = std::fs::read_to_string(&prev)
        .unwrap()
        .lines()
        .take_while(|l| !l.contains("\"turn_start\""))
        .map(String::from)
        .collect();
    std::fs::write(&prev, kept.join("\n") + "\n").unwrap();

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
    let recorded: SkillBundleManifest = serde_json::from_str(
        &std::fs::read_to_string(dir.path().join(".rupu/runs/run_two/skill_bundle.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(recorded, other);
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
