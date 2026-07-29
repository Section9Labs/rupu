//! Build identity embedded at compile time. The release build (see
//! `scripts/gh-build.sh`) exports `RUPU_RELEASE_CHANNEL` + `RUPU_RELEASE_VERSION`;
//! a local/dev build leaves them unset.

/// "beta" | "stable" for a published build; `None` for a dev build.
pub const RELEASE_CHANNEL: Option<&str> = option_env!("RUPU_RELEASE_CHANNEL");

/// The full release version (e.g. "0.35.4-beta" / "0.35.4"); falls back to the
/// crate version for dev builds.
pub const RELEASE_VERSION: &str = match option_env!("RUPU_RELEASE_VERSION") {
    Some(v) => v,
    None => env!("CARGO_PKG_VERSION"),
};

/// `"pkg"` when this binary came from a native package (.deb or .rpm);
/// `None` for tarball, `install.sh`, `cargo install`, and dev builds.
///
/// Stamped by the packaging build exactly the way `RUPU_RELEASE_CHANNEL` is
/// stamped by the release build — same mechanism, no new pattern.
///
/// Deliberately format-agnostic. Stamping "deb" vs "rpm" would need a
/// separate build per format; the distro tells us which is which at runtime
/// for free, because a .deb only installs on a Debian-family system.
pub const INSTALL_METHOD: Option<&str> = option_env!("RUPU_INSTALL_METHOD");

/// Whether this binary is owned by a system package manager.
///
/// Kept pure and separate from [`is_packaged`] so it is testable —
/// `option_env!` is fixed at compile time and cannot be varied from a test.
pub fn is_packaged_for(method: Option<&str>) -> bool {
    method == Some("pkg")
}

/// Whether this binary is owned by a system package manager.
pub fn is_packaged() -> bool {
    is_packaged_for(INSTALL_METHOD)
}

/// The fallback [`package_manager_hint`] value for a distro this binary
/// does not recognize (e.g. openSUSE, whose `ID_LIKE="opensuse suse"`
/// matches neither branch below). Callers that build a "run this command"
/// sentence must branch on this value rather than interpolating it
/// unconditionally — it is prose, not a package-manager name, and
/// `sudo your system package manager upgrade rupu` is not a real command.
pub const UNKNOWN_PACKAGE_MANAGER_HINT: &str = "your system package manager";

/// The upgrade command to name, derived from `/etc/os-release` contents.
///
/// Falls back to a true-but-vague phrase rather than naming the wrong
/// command: telling a user to run `apt` on a system without it is worse
/// than not naming a tool at all.
pub fn package_manager_hint_from(os_release: &str) -> &'static str {
    let s = os_release.to_ascii_lowercase();
    if s.contains("debian") || s.contains("ubuntu") {
        "apt"
    } else if s.contains("fedora")
        || s.contains("rhel")
        || s.contains("centos")
        || s.contains("rocky")
        || s.contains("almalinux")
    {
        "dnf"
    } else {
        UNKNOWN_PACKAGE_MANAGER_HINT
    }
}

/// The upgrade command to name for this machine.
pub fn package_manager_hint() -> &'static str {
    package_manager_hint_from(&std::fs::read_to_string("/etc/os-release").unwrap_or_default())
}

/// True when this binary was not built by the release tooling.
pub fn is_dev_build() -> bool {
    RELEASE_CHANNEL.is_none()
}

/// Human `--version` suffix, e.g. "0.35.4 (beta)" / "0.35.4 (dev)".
pub fn version_line() -> String {
    format!("{} ({})", RELEASE_VERSION, RELEASE_CHANNEL.unwrap_or("dev"))
}

/// `version_line()`, memoized behind a `'static` reference. clap's
/// `Command::version()` requires `impl IntoResettable<Str>`, which
/// `String` doesn't satisfy (only `&'static str` does) — this computes
/// the line once per process and hands back a `'static` borrow of that
/// single copy, so `rupu --version` can print the channel/version
/// without leaking a fresh allocation on every call.
pub fn version_line_static() -> &'static str {
    static CELL: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    CELL.get_or_init(version_line).as_str()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dev_build_when_env_absent() {
        // Under `cargo test` the release env is unset.
        assert!(is_dev_build());
        assert_eq!(RELEASE_CHANNEL, None);
        assert_eq!(RELEASE_VERSION, env!("CARGO_PKG_VERSION"));
        assert!(version_line().ends_with("(dev)"));
    }

    #[test]
    fn only_the_pkg_marker_counts_as_packaged() {
        assert!(is_packaged_for(Some("pkg")));
        // Tarball, install.sh, `cargo install`, and dev builds leave it
        // unset — none are owned by a package manager, so `rupu update`
        // must keep working for them.
        assert!(!is_packaged_for(None));
        assert!(!is_packaged_for(Some("")));
        assert!(!is_packaged_for(Some("tarball")));
    }

    #[test]
    fn package_manager_hint_follows_the_distro_family() {
        // A .deb only installs on a Debian-family system and an .rpm only on
        // an RHEL-family one, so the distro IS the answer — no per-format
        // build needed.
        assert_eq!(
            package_manager_hint_from("ID=debian\nNAME=\"Debian GNU/Linux\""),
            "apt"
        );
        assert_eq!(
            package_manager_hint_from("ID=ubuntu\nID_LIKE=debian"),
            "apt"
        );
        assert_eq!(package_manager_hint_from("ID=fedora"), "dnf");
        assert_eq!(
            package_manager_hint_from("ID=rocky\nID_LIKE=\"rhel centos fedora\""),
            "dnf"
        );
    }

    #[test]
    fn package_manager_hint_degrades_rather_than_guessing_wrong() {
        // Unknown or unreadable /etc/os-release: say something true rather
        // than name the wrong command.
        assert_eq!(package_manager_hint_from(""), "your system package manager");
        assert_eq!(
            package_manager_hint_from("ID=plan9"),
            "your system package manager"
        );
    }

    #[test]
    fn no_install_method_under_cargo_test() {
        assert_eq!(INSTALL_METHOD, None);
        assert!(!is_packaged());
    }
}
