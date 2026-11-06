//! Port: how `POST /api/skills/install` and `/update` fetch a git skill
//! source. The install itself is `rupu_skills::install_from` /
//! `update_skills`, in this crate; only the fetch is a port, because only
//! `rupu-cli` enables `rupu-skills`'s `git` feature (libgit2). `rupu cp
//! serve` installs `rupu_skills::git::GitFetcher`; `None` → the endpoints
//! return 501.

/// A git skill-source fetcher (`rupu_skills::source::Fetch`).
pub type SkillInstaller = dyn rupu_skills::source::Fetch + Send + Sync;
