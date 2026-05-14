use assert_cmd::Command;
use assert_fs::prelude::*;
use predicates::prelude::*;
use serde_json::Value;

#[test]
fn ui_themes_supports_json_and_lists_builtin_palette_and_syntax() {
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.child(".rupu");
    home.create_dir_all().unwrap();

    Command::cargo_bin("rupu")
        .unwrap()
        .env("RUPU_HOME", home.path())
        .current_dir(tmp.path())
        .args(["--format", "json", "ui", "themes"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"kind\": \"ui_theme_list\""))
        .stdout(predicate::str::contains("\"name\": \"rupu-dark\""))
        .stdout(predicate::str::contains("\"name\": \"base16-ocean.dark\""))
        .stdout(predicate::str::contains("\"name\": \"catppuccin-mocha\""))
        .stdout(predicate::str::contains("\"source\": \"builtin rupu\""));
}

#[test]
fn ui_theme_validate_supports_json() {
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.child(".rupu");
    home.create_dir_all().unwrap();
    let theme = tmp.child("custom-theme.toml");
    theme
        .write_str(
            r##"version = 1
name = "custom-night"
description = "Custom test theme"
base = "rupu-dark"
syntax_theme = "Solarized (dark)"

[palette]
brand = "#cba6f7"
separator = "#1e1e2e"
"##,
        )
        .unwrap();

    Command::cargo_bin("rupu")
        .unwrap()
        .env("RUPU_HOME", home.path())
        .current_dir(tmp.path())
        .args([
            "--format",
            "json",
            "ui",
            "theme",
            "validate",
            theme.path().to_str().unwrap(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"kind\": \"ui_theme_show\""))
        .stdout(predicate::str::contains("\"name\": \"custom-night\""))
        .stdout(predicate::str::contains(
            "\"syntax_theme\": \"Solarized (dark)\"",
        ));
}

#[test]
fn ui_theme_import_base16_installs_and_can_be_shown() {
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.child(".rupu");
    home.create_dir_all().unwrap();
    let source = tmp.child("tokyo-base16.yaml");
    source
        .write_str(
            r#"scheme: Tokyo Night Test
author: rupu
base00: "1a1b26"
base01: "16161e"
base02: "2f3549"
base03: "444b6a"
base04: "787c99"
base05: "a9b1d6"
base06: "cbccd1"
base07: "d5d6db"
base08: "f7768e"
base09: "ff9e64"
base0A: "e0af68"
base0B: "9ece6a"
base0C: "73daca"
base0D: "7aa2f7"
base0E: "bb9af7"
base0F: "c0caf5"
"#,
        )
        .unwrap();

    Command::cargo_bin("rupu")
        .unwrap()
        .env("RUPU_HOME", home.path())
        .current_dir(tmp.path())
        .args([
            "--format",
            "json",
            "ui",
            "theme",
            "import",
            source.path().to_str().unwrap(),
            "--from",
            "base16",
            "--name",
            "tokyo-base16-test",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"name\": \"tokyo-base16-test\""));

    home.child("themes/tokyo-base16-test.toml")
        .assert(predicate::path::exists());

    Command::cargo_bin("rupu")
        .unwrap()
        .env("RUPU_HOME", home.path())
        .current_dir(tmp.path())
        .args([
            "--format",
            "json",
            "ui",
            "theme",
            "show",
            "tokyo-base16-test",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"name\": \"tokyo-base16-test\""));
}

#[test]
fn ui_themes_lists_project_local_theme_and_marks_it_current() {
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.child(".rupu");
    home.create_dir_all().unwrap();
    let project = tmp.child("project");
    project.child(".rupu/themes").create_dir_all().unwrap();
    project
        .child(".rupu/config.toml")
        .write_str(
            r#"[ui.palette]
theme = "project-amber"
"#,
        )
        .unwrap();
    project
        .child(".rupu/themes/project-amber.toml")
        .write_str(
            r##"version = 1
name = "project-amber"
base = "rupu-dark"

[palette]
brand = "#ffb86c"
complete = "#f1fa8c"
"##,
        )
        .unwrap();

    Command::cargo_bin("rupu")
        .unwrap()
        .env("RUPU_HOME", home.path())
        .current_dir(project.path())
        .args(["--format", "json", "ui", "themes"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"name\": \"project-amber\""))
        .stdout(predicate::str::contains("\"source\": \"project file\""))
        .stdout(predicate::str::contains("\"current\": true"));
}

#[test]
fn ui_theme_legacy_selector_sets_matching_syntax_and_palette_theme() {
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.child(".rupu");
    home.create_dir_all().unwrap();
    let project = tmp.child("project");
    project.child(".rupu").create_dir_all().unwrap();
    project
        .child(".rupu/config.toml")
        .write_str(
            r#"[ui]
theme = "catppuccin-mocha"
"#,
        )
        .unwrap();

    let output = Command::cargo_bin("rupu")
        .unwrap()
        .env("RUPU_HOME", home.path())
        .current_dir(project.path())
        .args(["--format", "json", "ui", "themes"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    let rows = value["rows"].as_array().unwrap();
    let palette = rows
        .iter()
        .find(|row| row["kind"] == "palette" && row["name"] == "catppuccin-mocha")
        .unwrap();
    assert_eq!(palette["current"], true);
    let syntax = rows
        .iter()
        .find(|row| row["kind"] == "syntax" && row["name"] == "catppuccin-mocha")
        .unwrap();
    assert_eq!(syntax["current"], true);
}

#[test]
fn ui_theme_legacy_selector_maps_syntax_name_to_palette_alias() {
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.child(".rupu");
    home.create_dir_all().unwrap();
    let project = tmp.child("project");
    project.child(".rupu").create_dir_all().unwrap();
    project
        .child(".rupu/config.toml")
        .write_str(
            r#"[ui]
theme = "Solarized (dark)"
"#,
        )
        .unwrap();

    let output = Command::cargo_bin("rupu")
        .unwrap()
        .env("RUPU_HOME", home.path())
        .current_dir(project.path())
        .args(["--format", "json", "ui", "themes"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    let rows = value["rows"].as_array().unwrap();
    let palette = rows
        .iter()
        .find(|row| row["kind"] == "palette" && row["name"] == "solarized-dark")
        .unwrap();
    assert_eq!(palette["current"], true);
    let syntax = rows
        .iter()
        .find(|row| row["kind"] == "syntax" && row["name"] == "Solarized (dark)")
        .unwrap();
    assert_eq!(syntax["current"], true);
}
