use rupu_config::{layer_files, layer_files_locked, resolve, KeySource, LayerPaths, LockOwner};
use std::path::{Path, PathBuf};

fn write(dir: &Path, name: &str, body: &str) -> PathBuf {
    let p = dir.join(name);
    std::fs::write(&p, body).unwrap();
    p
}

struct Layers {
    _dir: tempfile::TempDir,
    g: PathBuf,
    c: PathBuf,
    p: PathBuf,
}

fn layers(g: &str, c: &str, p: &str) -> Layers {
    let dir = tempfile::tempdir().unwrap();
    Layers {
        g: write(dir.path(), "g.toml", g),
        c: write(dir.path(), "c.toml", c),
        p: write(dir.path(), "p.toml", p),
        _dir: dir,
    }
}

impl Layers {
    fn paths(&self) -> LayerPaths<'_> {
        LayerPaths::new(Some(&self.g), Some(&self.c), Some(&self.p))
    }
}

#[test]
fn customer_beats_global_when_project_is_silent() {
    let l = layers(
        "default_provider = \"anthropic\"\n",
        "default_provider = \"anthropic-acme\"\n",
        "",
    );
    let r = resolve(l.paths()).unwrap();
    assert_eq!(r.config.default_provider.as_deref(), Some("anthropic-acme"));
    let prov = &r.provenance["default_provider"];
    assert_eq!(prov.source, KeySource::Customer);
    assert!(!prov.locked);
    assert_eq!(prov.locked_by, None);
}

#[test]
fn project_beats_an_unlocked_customer_key() {
    let l = layers("", "default_model = \"c\"\n", "default_model = \"p\"\n");
    let r = resolve(l.paths()).unwrap();
    assert_eq!(r.config.default_model.as_deref(), Some("p"));
    assert_eq!(r.provenance["default_model"].source, KeySource::Project);
}

#[test]
fn a_customer_lock_beats_the_project() {
    let l = layers(
        "",
        "default_provider = \"anthropic-acme\"\n[policy]\nlock = [\"default_provider\"]\n",
        "default_provider = \"anthropic\"\n",
    );
    let r = resolve(l.paths()).unwrap();
    assert_eq!(r.config.default_provider.as_deref(), Some("anthropic-acme"));
    let prov = &r.provenance["default_provider"];
    assert_eq!(prov.source, KeySource::Customer);
    assert!(prov.locked);
    assert_eq!(prov.locked_by, Some(LockOwner::Customer));
    assert_eq!(r.customer_lock, vec!["default_provider".to_string()]);
    assert!(r.warnings.is_empty());
}

#[test]
fn a_global_lock_beats_a_customer_lock() {
    let l = layers(
        "permission_mode = \"readonly\"\n[policy]\nlock = [\"permission_mode\"]\n",
        "permission_mode = \"bypass\"\n[policy]\nlock = [\"permission_mode\"]\n",
        "permission_mode = \"ask\"\n",
    );
    let r = resolve(l.paths()).unwrap();
    assert_eq!(r.config.permission_mode.as_deref(), Some("readonly"));
    let prov = &r.provenance["permission_mode"];
    assert_eq!(prov.source, KeySource::Global);
    assert_eq!(prov.locked_by, Some(LockOwner::Global));
}

#[test]
fn the_resolved_lock_list_stays_global_only() {
    let l = layers(
        "[policy]\nlock = [\"log_level\"]\n",
        "default_model = \"c\"\n[policy]\nlock = [\"default_model\"]\n",
        "[policy]\nlock = []\n",
    );
    let r = resolve(l.paths()).unwrap();
    assert_eq!(r.config.policy.lock, vec!["log_level".to_string()]);
    assert_eq!(r.customer_lock, vec!["default_model".to_string()]);
}

#[test]
fn a_customer_lock_on_a_key_it_does_not_set_warns_and_locks_nothing() {
    let l = layers(
        "",
        "[policy]\nlock = [\"default_model\"]\n",
        "default_model = \"p\"\n",
    );
    let r = resolve(l.paths()).unwrap();
    assert_eq!(r.config.default_model.as_deref(), Some("p"));
    assert_eq!(r.provenance["default_model"].source, KeySource::Project);
    assert_eq!(r.provenance["default_model"].locked_by, None);
    assert_eq!(r.warnings.len(), 1, "{:?}", r.warnings);
    assert!(r.warnings[0].contains("default_model"));
}

#[test]
fn arrays_replace_across_three_layers() {
    let l = layers(
        "[[scm.rules]]\nowner = \"me\"\naccount = \"github\"\n",
        "[[scm.rules]]\nowner = \"acme-corp\"\naccount = \"github-acme\"\n",
        "",
    );
    let r = resolve(l.paths()).unwrap();
    assert_eq!(r.config.scm.rules.len(), 1);
    assert_eq!(r.config.scm.rules[0].account, "github-acme");
}

#[test]
fn layer_files_merges_three_layers_in_order_and_pins_the_global_lock() {
    let l = layers(
        "default_model = \"g\"\nlog_level = \"info\"\n[policy]\nlock = [\"log_level\"]\n",
        "default_model = \"c\"\ndefault_provider = \"anthropic-acme\"\n[policy]\nlock = [\"default_model\"]\n",
        "default_model = \"p\"\n",
    );
    let cfg = layer_files(l.paths()).unwrap();
    assert_eq!(cfg.default_model.as_deref(), Some("p"));
    assert_eq!(cfg.default_provider.as_deref(), Some("anthropic-acme"));
    assert_eq!(cfg.log_level.as_deref(), Some("info"));
    // Plain layering never trusts a lower layer's lock list.
    assert_eq!(cfg.policy.lock, vec!["log_level".to_string()]);
}

#[test]
fn layer_files_locked_honours_the_customer_lock() {
    let l = layers(
        "",
        "permission_mode = \"readonly\"\n[policy]\nlock = [\"permission_mode\"]\n",
        "permission_mode = \"bypass\"\n",
    );
    let cfg = layer_files_locked(l.paths()).unwrap();
    assert_eq!(cfg.permission_mode.as_deref(), Some("readonly"));
}

#[test]
fn a_missing_customer_file_is_an_empty_layer() {
    let dir = tempfile::tempdir().unwrap();
    let g = write(dir.path(), "g.toml", "default_model = \"g\"\n");
    let missing = dir.path().join("nope.toml");
    let cfg = layer_files(LayerPaths::new(Some(&g), Some(&missing), None)).unwrap();
    assert_eq!(cfg.default_model.as_deref(), Some("g"));
}

#[test]
fn a_malformed_customer_layer_is_an_error_naming_the_file() {
    let dir = tempfile::tempdir().unwrap();
    let c = write(dir.path(), "c.toml", "default_model = \n");
    let err = layer_files_locked(LayerPaths::new(None, Some(&c), None)).unwrap_err();
    assert!(err.to_string().contains("c.toml"), "{err}");
}

#[test]
fn provenance_serializes_customer_source_and_lock_owner() {
    let l = layers(
        "",
        "default_model = \"c\"\n[policy]\nlock = [\"default_model\"]\n",
        "",
    );
    let r = resolve(l.paths()).unwrap();
    let json = serde_json::to_value(&r.provenance["default_model"]).unwrap();
    assert_eq!(json["source"], "customer");
    assert_eq!(json["locked"], true);
    assert_eq!(json["locked_by"], "customer");
}

/// The dotted-key contract: a customer lock on a key whose segment holds a
/// `.` matches the canonical quoted encoding `resolve` uses for provenance.
#[test]
fn a_customer_lock_on_a_dotted_pricing_key_uses_the_canonical_encoding() {
    let l = layers(
        "",
        "[pricing.oracle.\"GLM-5.2-FP8\"]\ninput_per_mtok = 1.0\noutput_per_mtok = 2.0\n\
         [policy]\nlock = ['pricing.oracle.\"GLM-5.2-FP8\".input_per_mtok']\n",
        "[pricing.oracle.\"GLM-5.2-FP8\"]\ninput_per_mtok = 9.0\noutput_per_mtok = 9.0\n",
    );
    let r = resolve(l.paths()).unwrap();
    let locked = "pricing.oracle.\"GLM-5.2-FP8\".input_per_mtok";
    let free = "pricing.oracle.\"GLM-5.2-FP8\".output_per_mtok";
    assert_eq!(r.provenance[locked].source, KeySource::Customer);
    assert_eq!(r.provenance[locked].locked_by, Some(LockOwner::Customer));
    assert_eq!(r.provenance[free].source, KeySource::Project);
    let mp = r.config.pricing.models["oracle"]["GLM-5.2-FP8"];
    assert_eq!(mp.input_per_mtok, 1.0);
    assert_eq!(mp.output_per_mtok, 9.0);
}

#[test]
fn global_only_matches_the_old_single_file_load() {
    let dir = tempfile::tempdir().unwrap();
    let g = write(dir.path(), "g.toml", "default_model = \"g\"\n");
    let cfg = layer_files(LayerPaths::global_only(&g)).unwrap();
    assert_eq!(cfg.default_model.as_deref(), Some("g"));
}
