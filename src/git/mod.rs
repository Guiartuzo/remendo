//! The `git` CLI, behind a project-owned trait.
//!
//! Wider than it looks, because two decisions put work here that could have
//! gone elsewhere (design.md §13/§14):
//!
//! * **Credentials.** Remendo stores no secret and parses no `.netrc`; it asks
//!   `git credential fill`, inheriting whatever already works for `git push`.
//!   That makes credentials git's concern, not Gerrit's.
//! * **The CA hint.** When TLS fails, git's own `http.sslCAInfo` is where a
//!   corporate CA is configured for everyone whose `git push` works.

pub mod cli;
pub mod fake;

use std::path::{Path, PathBuf};

pub use cli::GitCommand;
pub use fake::FakeGit;

/// A username/password pair from git's credential helper.
///
/// The password is not logged or displayed anywhere; `Debug` is implemented by
/// hand so it cannot leak into a panic message or a structured log line.
#[derive(Clone, PartialEq, Eq)]
pub struct Credential {
    pub username: String,
    pub password: String,
}

impl std::fmt::Debug for Credential {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Credential")
            .field("username", &self.username)
            .field("password", &"<redacted>")
            .finish()
    }
}

/// The git operations Remendo needs.
///
/// Implemented by [`GitCommand`] over the real CLI and by [`FakeGit`] in tests,
/// per `config.yaml`'s rule that external I/O is mocked with a named fake type
/// rather than an ad-hoc stub.
pub trait GitCli {
    /// The root of the clone containing `cwd`, or an error when there is none.
    /// `remendo <change-id>` requires this: a change id names no repository.
    fn repo_root(&self) -> Result<PathBuf, GitError>;

    /// A remote's URL, used to derive the Gerrit base URL.
    fn remote_url(&self, remote: &str) -> Result<String, GitError>;

    /// A git config value, or `None` when the key is simply unset.
    ///
    /// `git config --get` exits non-zero for an absent key, which is not a
    /// failure — so an unset key is `Ok(None)`, not `Err`.
    fn config_get(&self, key: &str) -> Result<Option<String>, GitError>;

    /// A credential for `host` via git's credential protocol.
    fn fill_credential(&self, host: &str) -> Result<Credential, GitError>;

    /// Fetch `refspec` from `remote` into the clone.
    fn fetch(&self, remote: &str, refspec: &str) -> Result<(), GitError>;

    /// Create a worktree at `path` with `revision` checked out (detached).
    fn worktree_add(&self, path: &Path, revision: &str) -> Result<(), GitError>;

    /// Stage one path inside `worktree`. Staging is explicit and happens per
    /// confirm, so "confirmed" and "staged" stay the same set (design.md §8).
    fn stage(&self, worktree: &Path, path: &str) -> Result<(), GitError>;

    /// Amend the staged changes into the checked-out commit. `message` is
    /// `Some` only when a `/COMMIT_MSG` comment was accepted — so this is
    /// deliberately not a blanket `--amend --no-edit`.
    fn commit_amend(&self, worktree: &Path, message: Option<&str>) -> Result<(), GitError>;

    /// Push from `worktree` using `refspec` (`HEAD:refs/for/<branch>`).
    fn push(&self, worktree: &Path, refspec: &str) -> Result<(), GitError>;
}

/// Failures running git.
///
/// Every variant names the offending value, per `config.yaml`.
#[derive(Debug, thiserror::Error)]
pub enum GitError {
    #[error("`git` is not on PATH — Remendo drives the git CLI and cannot run without it")]
    GitNotFound,

    #[error(
        "not inside a git clone (cwd: {cwd}). `remendo <change-id>` must run from within a \
         clone of the change's project — a change id does not identify a repository."
    )]
    NotAClone { cwd: PathBuf },

    #[error("remote `{remote}` has no URL configured in this clone")]
    NoSuchRemote { remote: String },

    #[error(
        "no credential available for host `{host}`. Remendo asks git's credential helper, \
         so whatever authenticates `git push` to this host should work here too."
    )]
    NoCredential { host: String },

    #[error("`git {command}` failed with status {status}: {stderr}")]
    CommandFailed {
        command: String,
        status: String,
        stderr: String,
    },

    #[error("could not run `git {command}`: {source}")]
    Spawn {
        command: String,
        source: std::io::Error,
    },
}
