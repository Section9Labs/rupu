use std::io::Write;
use std::path::{Path, PathBuf};

/// Self-ignoring `.gitignore` dropped into the netflow ledger directory.
/// A bare `*` ignores everything in this directory, including itself,
/// regardless of whether the enclosing project's own `.gitignore` (or
/// `rupu init`'s template) ever mentions `.rupu/netflow/` — see
/// `ensure_dir` for why this is the load-bearing protection, not that
/// project-level entry.
const NETFLOW_GITIGNORE: &str = "\
# rupu netflow ledger: every host, IP, path, and timing rupu contacted
# during a run. Local diagnostics only — never commit this directory.
*
";

/// Canonical on-disk layout of a workspace's netflow ledger.
#[derive(Debug, Clone)]
pub struct NetflowPaths {
    pub root: PathBuf,
    pub flows: PathBuf,
}

impl NetflowPaths {
    /// One ledger per run, mirroring how transcripts are laid out.
    ///
    /// `netflow_dir` comes from `rupu_cli::paths::netflow_dir`, which
    /// resolves project-local-when-present with a global fallback. The
    /// per-run file is what makes a ledger's lifecycle match a
    /// transcript's: it ends when the run ends, so there is nothing to
    /// rotate, and the file itself is the run attribution.
    pub fn for_run(netflow_dir: &Path, run_id: &str) -> Self {
        Self {
            root: netflow_dir.to_path_buf(),
            flows: netflow_dir.join(format!("{run_id}.jsonl")),
        }
    }

    /// Create the ledger directory and make sure it is self-ignoring.
    ///
    /// The directory-level `.gitignore` is the actual privacy boundary:
    /// project-level protection (this repo's own `.rupu/.gitignore`,
    /// `rupu init`'s `GITIGNORE_ENTRIES`) only reaches projects
    /// initialised at or after those changes landed. This runs
    /// unconditionally for every caller — including projects that were
    /// never `rupu init`'d, or were initialised by an older binary — so
    /// the protection travels with the directory itself.
    ///
    /// Deliberately fails closed rather than open: if the directory can
    /// be created but the `.gitignore` cannot be written, this returns
    /// that error instead of proceeding as if nothing happened. A ledger
    /// that silently opens with no ignore file is a privacy leak waiting
    /// for the user's next `git add .`; the caller
    /// (`NetflowWriterHandle::spawn`) already treats an `ensure_dir`
    /// error as best-effort and degrades to transcript-only capture
    /// (see `run.rs` / `cmd/cp.rs`), so losing the ledger here is
    /// strictly better than leaking it. Do not "simplify" this into a
    /// best-effort write that swallows the error.
    pub fn ensure_dir(&self) -> std::io::Result<()> {
        std::fs::create_dir_all(&self.root)?;
        self.ensure_self_ignore()
    }

    /// Write the self-ignoring `.gitignore`, but only if one is not
    /// already there — never clobber a file the user may have
    /// customised. Uses `create_new` so the presence check and the write
    /// are atomic (no separate `exists()` + `write()` TOCTOU window);
    /// an `AlreadyExists` error means the file is already there, which
    /// is success for our purposes, not a failure to propagate.
    ///
    /// Note this intentionally does *not* re-validate or heal an
    /// existing `.gitignore`, even one that is empty or lacks the `*`
    /// pattern (e.g. a user deliberately cleared it): requirement 1 is
    /// "never clobber a file the user may have customised", and an
    /// empty file is a valid customisation, not corruption, from
    /// `ensure_dir`'s point of view. See `write_all`'s error arm below
    /// for the one case this crate itself can produce that *does* need
    /// cleaning up.
    fn ensure_self_ignore(&self) -> std::io::Result<()> {
        let gitignore = self.root.join(".gitignore");
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&gitignore)
        {
            Ok(mut file) => {
                if let Err(e) = file.write_all(NETFLOW_GITIGNORE.as_bytes()) {
                    // `create_new` already succeeded, so a 0-byte (or
                    // partially written) file is sitting on disk. Left
                    // alone, every future call would hit `AlreadyExists`
                    // below and treat that corpse as "already
                    // protected" forever — silently reintroducing the
                    // exact leak this function exists to close, just
                    // one layer down. Best-effort delete it so the next
                    // call retries `create_new` cleanly; if the cleanup
                    // itself fails there's nothing further to do, so
                    // its result is deliberately ignored and the
                    // original write error is what gets surfaced.
                    let _ = std::fs::remove_file(&gitignore);
                    return Err(e);
                }
                Ok(())
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
            Err(e) => Err(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn for_run_puts_each_run_in_its_own_file() {
        let tmp = tempfile::TempDir::new().unwrap();
        let a = NetflowPaths::for_run(tmp.path(), "run-a");
        let b = NetflowPaths::for_run(tmp.path(), "run-b");

        assert_eq!(a.root, tmp.path());
        assert_eq!(a.flows, tmp.path().join("run-a.jsonl"));
        assert_eq!(b.flows, tmp.path().join("run-b.jsonl"));
        assert_ne!(a.flows, b.flows, "two runs must never share a ledger");
    }

    #[test]
    fn ensure_dir_is_idempotent() {
        let tmp = tempfile::TempDir::new().unwrap();
        let paths = NetflowPaths::for_run(tmp.path(), "run-1");
        paths.ensure_dir().unwrap();
        paths.ensure_dir().unwrap();
        assert!(paths.root.is_dir());
    }

    #[test]
    fn ensure_dir_writes_self_ignoring_gitignore() {
        let tmp = tempfile::TempDir::new().unwrap();
        let paths = NetflowPaths::for_run(tmp.path(), "run-1");
        paths.ensure_dir().unwrap();

        let gitignore = paths.root.join(".gitignore");
        let contents = std::fs::read_to_string(&gitignore).unwrap();
        assert!(
            contents.lines().any(|line| line.trim() == "*"),
            "expected a bare `*` pattern to ignore everything under the netflow dir, got: {contents:?}"
        );
    }

    #[test]
    fn ensure_dir_does_not_rewrite_gitignore_on_second_call() {
        let tmp = tempfile::TempDir::new().unwrap();
        let paths = NetflowPaths::for_run(tmp.path(), "run-1");
        paths.ensure_dir().unwrap();

        let gitignore = paths.root.join(".gitignore");
        let first = std::fs::read_to_string(&gitignore).unwrap();

        paths.ensure_dir().unwrap();
        let second = std::fs::read_to_string(&gitignore).unwrap();

        assert_eq!(
            first, second,
            "second ensure_dir call must not rewrite the .gitignore"
        );
    }

    #[test]
    fn ensure_dir_leaves_a_preexisting_customised_gitignore_untouched() {
        let tmp = tempfile::TempDir::new().unwrap();
        let paths = NetflowPaths::for_run(tmp.path(), "run-1");
        std::fs::create_dir_all(&paths.root).unwrap();
        let gitignore = paths.root.join(".gitignore");
        std::fs::write(&gitignore, "# user customised this\n!flows.jsonl\n").unwrap();

        paths.ensure_dir().unwrap();

        let contents = std::fs::read_to_string(&gitignore).unwrap();
        assert_eq!(contents, "# user customised this\n!flows.jsonl\n");
    }

    /// An existing but empty (or otherwise pattern-less) `.gitignore` is a
    /// distinct case from a customised one, but gets the same treatment:
    /// `ensure_dir` does not heal or rewrite it. A user may have
    /// deliberately emptied the file, and clobbering user files is exactly
    /// what requirement 1 forbids — `ensure_self_ignore`'s `remove_file`
    /// cleanup only ever fires on a file *this crate* just created and
    /// failed to finish writing, never on a pre-existing file it didn't
    /// create.
    #[test]
    fn ensure_dir_leaves_a_preexisting_empty_gitignore_untouched() {
        let tmp = tempfile::TempDir::new().unwrap();
        let paths = NetflowPaths::for_run(tmp.path(), "run-1");
        std::fs::create_dir_all(&paths.root).unwrap();
        let gitignore = paths.root.join(".gitignore");
        std::fs::write(&gitignore, "").unwrap();

        paths.ensure_dir().unwrap();

        let contents = std::fs::read_to_string(&gitignore).unwrap();
        assert_eq!(
            contents, "",
            "a pre-existing empty .gitignore must not be rewritten"
        );
    }

    /// The strongest test of the actual property: run `ensure_dir` inside a
    /// real git repo, drop a `flows.jsonl` in the ledger dir exactly like the
    /// writer would, and ask git itself whether it would be tracked. Shelling
    /// out to `git` in tests is an established pattern in this workspace
    /// (`crates/rupu-workspace/src/store.rs`, `crates/rupu-workspace/src/autoflow_worktree.rs`,
    /// `crates/rupu-cli/tests/cli_run.rs`, `crates/rupu-cli/src/cmd/init.rs`),
    /// so this is exercised directly rather than faked.
    #[test]
    fn flows_jsonl_is_git_ignored_in_a_real_repo() {
        let tmp = tempfile::TempDir::new().unwrap();
        let status = std::process::Command::new("git")
            .arg("init")
            .arg(tmp.path())
            .status()
            .unwrap();
        assert!(status.success());

        let paths = NetflowPaths::for_run(tmp.path(), "run-1");
        paths.ensure_dir().unwrap();
        std::fs::write(&paths.flows, "{}\n").unwrap();

        let output = std::process::Command::new("git")
            .arg("-C")
            .arg(tmp.path())
            .arg("check-ignore")
            .arg(&paths.flows)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "expected `git check-ignore` to report {:?} as ignored; stdout={:?} stderr={:?}",
            paths.flows,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
    }
}
