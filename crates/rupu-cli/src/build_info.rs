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

/// Who owns this binary, when it was not placed by hand.
///
/// [`INSTALL_METHOD`] only covers the formats rupu itself builds (.deb/.rpm).
/// Homebrew, the AUR and Nix all install the *plain* published binary, so they
/// carry no marker — yet they own the file just as much, and self-updating one
/// fights the manager that owns it (brew/pacman revert it on the next upgrade;
/// the Nix store is read-only). Hence the second, path-based signal below.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallOwner {
    /// Homebrew, on macOS or Linux. Never takes `sudo`.
    Brew,
    /// The Nix store — immutable by construction.
    Nix,
    /// A distro package manager: apt / dnf / pacman (.deb, .rpm, AUR).
    Distro,
}

/// Which package manager owns a binary living at `path`, if any.
///
/// `/usr/local/bin` is deliberately **absent**: that is exactly where the docs
/// tell you to `mv` a downloaded binary, and those installs *should* keep
/// self-updating. The Intel Homebrew prefix is matched via its `Cellar` keg
/// path for the same reason — so a hand-placed `/usr/local/bin/rupu` is never
/// mistaken for a brew install.
///
/// Pure so the whole matrix is testable; the real path comes from
/// [`installed_owner`].
pub fn owner_for_path(path: &str) -> Option<InstallOwner> {
    if path.starts_with("/nix/store/") {
        return Some(InstallOwner::Nix);
    }
    if path.starts_with("/opt/homebrew/")
        || path.starts_with("/home/linuxbrew/")
        || path.contains("/Cellar/")
    {
        return Some(InstallOwner::Brew);
    }
    // Distro territory on Linux (pacman/AUR land here, as do .deb/.rpm).
    if path.starts_with("/usr/bin/") {
        return Some(InstallOwner::Distro);
    }
    None
}

/// Who owns *this* binary: the compile-time package marker first, then the
/// path it is running from. `None` means a hand-placed or `cargo install`
/// binary that is free to replace itself.
pub fn installed_owner() -> Option<InstallOwner> {
    if is_packaged_for(INSTALL_METHOD) {
        return Some(InstallOwner::Distro);
    }
    let exe = std::env::current_exe().ok()?;
    let exe = std::fs::canonicalize(&exe).unwrap_or(exe);
    owner_for_path(&exe.to_string_lossy())
}

/// The exact upgrade command to name for `owner`, or `None` when no single
/// command is correct — the caller must then fall back to prose rather than
/// print something uncopy-pasteable.
///
/// Note `brew` is returned **without** `sudo`: Homebrew refuses to run under
/// sudo, so `sudo brew upgrade` is actively wrong advice. Nix has no single
/// upgrade command (it depends on profile vs flake vs NixOS module), so it
/// returns `None` on purpose.
pub fn upgrade_command(owner: InstallOwner, distro_pm: &str) -> Option<String> {
    match owner {
        InstallOwner::Brew => Some("brew upgrade rupu".to_string()),
        InstallOwner::Nix => None,
        InstallOwner::Distro if distro_pm != UNKNOWN_PACKAGE_MANAGER_HINT => {
            Some(format!("sudo {distro_pm} upgrade rupu"))
        }
        InstallOwner::Distro => None,
    }
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
    fn package_manager_owned_paths_are_recognized() {
        // Homebrew: Apple Silicon, Linuxbrew, and any Cellar keg.
        assert_eq!(
            owner_for_path("/opt/homebrew/bin/rupu"),
            Some(InstallOwner::Brew)
        );
        assert_eq!(
            owner_for_path("/home/linuxbrew/.linuxbrew/bin/rupu"),
            Some(InstallOwner::Brew)
        );
        assert_eq!(
            owner_for_path("/usr/local/Cellar/rupu/0.74.0/bin/rupu"),
            Some(InstallOwner::Brew)
        );
        // Nix store paths are immutable.
        assert_eq!(
            owner_for_path("/nix/store/abc123-rupu-0.74.0/bin/rupu"),
            Some(InstallOwner::Nix)
        );
        // Distro territory — where pacman/AUR and .deb/.rpm land.
        assert_eq!(owner_for_path("/usr/bin/rupu"), Some(InstallOwner::Distro));
    }

    #[test]
    fn hand_placed_binaries_stay_self_updatable() {
        // The documented manual install location must NOT be treated as
        // package-owned, or `rupu update` refuses the very install path the
        // docs tell people to use.
        assert_eq!(owner_for_path("/usr/local/bin/rupu"), None);
        assert_eq!(owner_for_path("/home/matt/.cargo/bin/rupu"), None);
        assert_eq!(owner_for_path("/home/matt/bin/rupu"), None);
        assert_eq!(owner_for_path("./target/release/rupu"), None);
    }

    #[test]
    fn upgrade_command_never_tells_you_to_sudo_brew() {
        // Homebrew refuses to run under sudo — naming it would be wrong.
        assert_eq!(
            upgrade_command(InstallOwner::Brew, "apt").as_deref(),
            Some("brew upgrade rupu")
        );
        assert_eq!(
            upgrade_command(InstallOwner::Distro, "apt").as_deref(),
            Some("sudo apt upgrade rupu")
        );
        assert_eq!(
            upgrade_command(InstallOwner::Distro, "dnf").as_deref(),
            Some("sudo dnf upgrade rupu")
        );
        // Nix has no single upgrade command; an unknown distro has no name.
        assert_eq!(upgrade_command(InstallOwner::Nix, "apt"), None);
        assert_eq!(
            upgrade_command(InstallOwner::Distro, UNKNOWN_PACKAGE_MANAGER_HINT),
            None
        );
    }

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
