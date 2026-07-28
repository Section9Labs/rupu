use semver::Version;
use std::path::Path;

/// Map a Rust `(os, arch)` pair to rupu's canonical release-asset platform
/// name.
///
/// Kept pure — rather than reading `std::env::consts` inline — so the whole
/// mapping table is testable from any host. This function is the single
/// owner of the asset-naming convention: `scripts/gh-build.sh` and the
/// release workflow both read it back out of the built binary via
/// `rupu update --print-platform` rather than deriving names independently.
///
/// A second, independent derivation is exactly the defect this replaces:
/// `uname -m` yields `x86_64` where this yields `x64`, so a publisher using
/// `uname` would write `rupu-linux-x86_64` while `rupu update` looked for
/// `rupu-linux-x64`. They agree only by coincidence on Apple Silicon.
pub fn platform_name(os: &str, arch: &str) -> String {
    let os = match os {
        "macos" => "darwin",
        other => other,
    };
    let arch = match arch {
        "aarch64" => "arm64",
        "x86_64" => "x64",
        other => other,
    };
    format!("{os}-{arch}")
}

/// `<os>-<arch>` for the host this binary is running on.
pub fn current_platform() -> String {
    platform_name(std::env::consts::OS, std::env::consts::ARCH)
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
    fn platform_name_maps_every_published_target() {
        assert_eq!(platform_name("macos", "aarch64"), "darwin-arm64");
        assert_eq!(platform_name("linux", "x86_64"), "linux-x64");
        assert_eq!(platform_name("linux", "aarch64"), "linux-arm64");
    }

    #[test]
    fn platform_name_maps_targets_we_can_build_but_do_not_publish() {
        assert_eq!(platform_name("macos", "x86_64"), "darwin-x64");
    }

    #[test]
    fn platform_name_passes_unknown_pairs_through_unchanged() {
        // Not published, but the mapping must not silently mangle a
        // target we never anticipated — a wrong-but-plausible name is
        // worse than an obviously-unsupported one.
        assert_eq!(platform_name("freebsd", "riscv64"), "freebsd-riscv64");
    }

    /// The defect this whole contract exists to prevent: `uname -m`
    /// reports `x86_64` where this mapping says `x64`. Anything deriving
    /// the asset name independently will disagree on every x86_64 host.
    #[test]
    fn arch_mapping_differs_from_uname_which_is_why_it_has_one_owner() {
        assert_ne!(platform_name("linux", "x86_64"), "linux-x86_64");
        assert_eq!(platform_name("linux", "x86_64"), "linux-x64");
    }

    #[test]
    fn current_platform_delegates_to_the_pure_mapping() {
        assert_eq!(
            current_platform(),
            platform_name(std::env::consts::OS, std::env::consts::ARCH)
        );
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
