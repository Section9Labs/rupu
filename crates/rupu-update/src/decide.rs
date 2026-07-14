use semver::Version;
use std::path::Path;

/// `<os>-<arch>`, mapping Rust's arch names to our asset convention.
pub fn current_platform() -> String {
    let os = std::env::consts::OS; // "macos", "linux"
    let os = match os {
        "macos" => "darwin",
        other => other,
    };
    let arch = match std::env::consts::ARCH {
        "aarch64" => "arm64",
        "x86_64" => "x64",
        other => other,
    };
    format!("{os}-{arch}")
}

/// True when the running binary is a dev build (path under a `target/` build dir).
pub fn is_dev_exe(exe_path: &Path) -> bool {
    let comps: Vec<_> = exe_path
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect();
    comps
        .windows(2)
        .any(|w| w[0] == "target" && (w[1] == "debug" || w[1] == "release"))
}

#[derive(Debug, PartialEq)]
pub enum Decision {
    UpToDate,
    Update { from: Version, to: Version },
    Ahead,
}

pub fn decide(current: &Version, latest: &Version, force: bool) -> Decision {
    use std::cmp::Ordering::*;
    match latest.cmp(current) {
        Greater => Decision::Update {
            from: current.clone(),
            to: latest.clone(),
        },
        Equal => {
            if force {
                Decision::Update {
                    from: current.clone(),
                    to: latest.clone(),
                }
            } else {
                Decision::UpToDate
            }
        }
        Less => {
            if force {
                Decision::Update {
                    from: current.clone(),
                    to: latest.clone(),
                }
            } else {
                Decision::Ahead
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn v(s: &str) -> Version {
        Version::parse(s).unwrap()
    }

    #[test]
    fn newer_triggers_update() {
        assert_eq!(
            decide(&v("0.35.3"), &v("0.35.4"), false),
            Decision::Update {
                from: v("0.35.3"),
                to: v("0.35.4")
            }
        );
    }
    #[test]
    fn equal_is_up_to_date_unless_forced() {
        assert_eq!(
            decide(&v("0.35.4"), &v("0.35.4"), false),
            Decision::UpToDate
        );
        assert!(matches!(
            decide(&v("0.35.4"), &v("0.35.4"), true),
            Decision::Update { .. }
        ));
    }
    #[test]
    fn older_latest_is_ahead() {
        assert_eq!(decide(&v("0.35.5"), &v("0.35.4"), false), Decision::Ahead);
    }
    #[test]
    fn dev_exe_detected_under_target() {
        assert!(is_dev_exe(Path::new("/x/rupu/target/release/rupu")));
        assert!(!is_dev_exe(Path::new("/usr/local/bin/rupu")));
    }
}
