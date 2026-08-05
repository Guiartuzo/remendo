//! Opening a review: validate, then check the patchset out in isolation.
//!
//! The order of operations is the requirement, not an implementation detail.
//! Every check that can fail happens **before** anything is created, so a bad
//! launch leaves no directory behind to confuse the next one
//! (specs/review-workspace).
//!
//! ```text
//!   remendo <change-id>
//!      │
//!      ├─ cwd inside a clone?      no ─▶ error, nothing created
//!      ├─ change exists/visible?   no ─▶ error, nothing created
//!      ├─ project matches clone?   no ─▶ error naming BOTH, nothing created
//!      │
//!      ├─ worktree already there?  yes ─▶ RESUME (design.md §13)
//!      └─                          no  ─▶ fetch + worktree add
//! ```

pub mod cache;
pub mod paths;

use std::path::{Path, PathBuf};

pub use cache::VerdictCache;
pub use paths::SessionPaths;

use crate::gerrit::api::{ChangeInfo, GerritApi};
use crate::gerrit::{GerritError, base_url};
use crate::git::{GitCli, GitError};

/// An opened review session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Workspace {
    /// The change under review.
    pub change: ChangeInfo,
    /// Where this session's files live.
    pub paths: SessionPaths,
    /// Whether this launch reused an existing worktree rather than creating one.
    pub resumed: bool,
}

impl Workspace {
    /// The worktree the patchset is checked out in — the path the user needs
    /// for the fix-in-your-own-editor path, so it is available throughout the
    /// review rather than only on abort.
    pub fn worktree(&self) -> PathBuf {
        self.paths.worktree()
    }

    /// Load this change's cached verdicts, if they describe its current
    /// revision. A new patchset is a miss.
    pub fn load_verdicts(&self) -> Option<VerdictCache> {
        VerdictCache::load(
            &self.paths.verdicts(),
            &self.change.id,
            &self.change.current_revision,
        )
    }

    /// A cache to fill for this change's current revision.
    pub fn new_verdict_cache(&self) -> VerdictCache {
        VerdictCache::new(&self.change.id, &self.change.current_revision)
    }

    /// What to tell the user when they abort: the worktree survives with its
    /// confirmed edits, nothing was pushed, and here is where it is.
    pub fn abort_report(&self) -> String {
        format!(
            "Nothing was pushed. The worktree and any confirmed edits are left at:\n  {}",
            self.worktree().display()
        )
    }
}

/// Open a review for `change_id`, validating before creating anything.
///
/// `state_dir` is where sessions live — [`paths::state_dir`] resolves the real
/// one; tests pass a temporary directory.
pub fn open(
    git: &impl GitCli,
    api: &impl GerritApi,
    change_id: &str,
    state_dir: &Path,
) -> Result<Workspace, WorkspaceError> {
    // 1. cwd must be inside a clone: a change id names no repository.
    git.repo_root()?;
    let origin = git.remote_url("origin")?;

    // 2. The change must exist and be visible.
    let change = api.change(change_id)?;

    // 3. The clone must be the change's project.
    let clone_project =
        base_url::project_of(&origin).ok_or_else(|| WorkspaceError::UndeterminedProject {
            url: origin.clone(),
        })?;
    if clone_project != change.project {
        return Err(WorkspaceError::ProjectMismatch {
            change_project: change.project.clone(),
            clone_project,
        });
    }

    // Only now may anything be created.
    let paths = SessionPaths::new(state_dir, &change.project, change_id)?;
    let resumed = paths.worktree_exists();
    if !resumed {
        create_worktree(git, &change, &paths)?;
    }
    Ok(Workspace {
        change,
        paths,
        resumed,
    })
}

/// Fetch the change's current revision and check it out into a new worktree.
fn create_worktree(
    git: &impl GitCli,
    change: &ChangeInfo,
    paths: &SessionPaths,
) -> Result<(), WorkspaceError> {
    let revision_ref =
        change
            .current_revision_ref()
            .ok_or_else(|| WorkspaceError::NoRevisionRef {
                change_id: change.id.clone(),
                revision: change.current_revision.clone(),
            })?;

    std::fs::create_dir_all(&paths.root).map_err(|source| WorkspaceError::Io {
        path: paths.root.clone(),
        source,
    })?;
    git.fetch("origin", revision_ref)?;
    // Detached at the revision: the worktree has no local branch, which is why
    // finalize reads the push target from the change's `branch` field.
    git.worktree_add(&paths.worktree(), &change.current_revision)?;
    Ok(())
}

/// Failures opening or resuming a review.
#[derive(Debug, thiserror::Error)]
pub enum WorkspaceError {
    #[error(transparent)]
    Git(#[from] GitError),

    #[error(transparent)]
    Gerrit(#[from] GerritError),

    #[error(
        "change belongs to project `{change_project}`, but this clone is `{clone_project}`. \
         Run remendo from a clone of the change's project."
    )]
    ProjectMismatch {
        change_project: String,
        clone_project: String,
    },

    #[error("could not determine this clone's project from its origin remote `{url}`")]
    UndeterminedProject { url: String },

    #[error(
        "change `{change_id}` revision `{revision}` has no fetchable ref — \
         Gerrit returned no `ref` for the current revision"
    )]
    NoRevisionRef { change_id: String, revision: String },

    #[error("unsafe {what} `{value}`: it would place the session outside the state directory")]
    UnsafeName { what: String, value: String },

    #[error("no state directory: neither XDG_STATE_HOME nor HOME is set")]
    NoStateDir,

    #[error("could not access {path}: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gerrit::FakeGerrit;
    use crate::git::{FakeGit, GitCall};

    const ORIGIN: &str = "https://gerrit.corp/a/platform/base";

    const CHANGE_BODY: &str = r#")]}'
{
  "id": "12345",
  "project": "platform/base",
  "branch": "main",
  "current_revision": "d3adb33f",
  "revisions": {"d3adb33f": {"_number": 3, "ref": "refs/changes/45/12345/3"}}
}"#;

    fn gerrit() -> FakeGerrit {
        FakeGerrit::from_change_json(CHANGE_BODY).unwrap()
    }

    fn state_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("remendo-ws-test-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn opening_validates_then_creates_the_worktree() {
        let dir = state_dir("open");
        let git = FakeGit::in_clone("/repo", ORIGIN);
        let ws = open(&git, &gerrit(), "12345", &dir).unwrap();

        assert!(!ws.resumed);
        assert_eq!(ws.change.branch, "main");
        assert_eq!(
            git.calls(),
            vec![
                GitCall::Fetch {
                    remote: "origin".into(),
                    refspec: "refs/changes/45/12345/3".into()
                },
                GitCall::WorktreeAdd {
                    path: ws.worktree(),
                    revision: "d3adb33f".into()
                },
            ]
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The requirement is the ordering: a launch that cannot succeed must not
    /// leave a directory behind for the next one to trip over.
    #[test]
    fn nothing_is_created_when_the_change_is_unknown() {
        let dir = state_dir("unknown-change");
        let git = FakeGit::in_clone("/repo", ORIGIN);
        let err = open(&git, &FakeGerrit::default(), "98765", &dir).unwrap_err();

        assert!(matches!(err, WorkspaceError::Gerrit(_)));
        assert!(
            git.calls().is_empty(),
            "no git work before validation passes"
        );
        assert!(!dir.exists(), "no directory left behind");
    }

    #[test]
    fn nothing_is_created_outside_a_clone() {
        let dir = state_dir("no-clone");
        let git = FakeGit::outside_a_clone();
        let err = open(&git, &gerrit(), "12345", &dir).unwrap_err();

        assert!(matches!(
            err,
            WorkspaceError::Git(GitError::NotAClone { .. })
        ));
        assert!(!dir.exists());
    }

    /// config.yaml requires the offending value in the message; a mismatch has
    /// two, and reporting only one leaves the user guessing which is wrong.
    #[test]
    fn a_project_mismatch_names_both_projects() {
        let dir = state_dir("mismatch");
        let git = FakeGit::in_clone("/repo", "https://gerrit.corp/a/other/repo");
        let err = open(&git, &gerrit(), "12345", &dir).unwrap_err();

        let msg = err.to_string();
        assert!(msg.contains("platform/base"), "names the change's: {msg}");
        assert!(msg.contains("other/repo"), "names the clone's: {msg}");
        assert!(git.calls().is_empty());
        assert!(!dir.exists());
    }

    /// The second run against any change hits this, because abort deliberately
    /// leaves the worktree behind.
    #[test]
    fn a_relaunch_resumes_instead_of_recreating() {
        let dir = state_dir("resume");
        let git = FakeGit::in_clone("/repo", ORIGIN);

        let first = open(&git, &gerrit(), "12345", &dir).unwrap();
        assert!(!first.resumed);
        // FakeGit records rather than creates, so make the worktree real.
        std::fs::create_dir_all(first.worktree()).unwrap();

        let git2 = FakeGit::in_clone("/repo", ORIGIN);
        let second = open(&git2, &gerrit(), "12345", &dir).unwrap();
        assert!(second.resumed);
        assert!(
            git2.calls().is_empty(),
            "a resume must not re-fetch or re-add the worktree"
        );
        assert_eq!(first.paths, second.paths);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_change_with_no_fetchable_ref_is_reported_by_name() {
        let dir = state_dir("no-ref");
        let body = r#")]}'
{"id":"12345","project":"platform/base","branch":"main","current_revision":"sha"}"#;
        let git = FakeGit::in_clone("/repo", ORIGIN);
        let err = open(
            &git,
            &FakeGerrit::from_change_json(body).unwrap(),
            "12345",
            &dir,
        )
        .unwrap_err();

        let msg = err.to_string();
        assert!(msg.contains("12345") && msg.contains("sha"), "{msg}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_worktree_path_is_available_for_the_abort_report() {
        let dir = state_dir("abort");
        let git = FakeGit::in_clone("/repo", ORIGIN);
        let ws = open(&git, &gerrit(), "12345", &dir).unwrap();

        let report = ws.abort_report();
        assert!(report.contains(&ws.worktree().display().to_string()));
        assert!(report.contains("Nothing was pushed"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn verdicts_survive_a_relaunch_at_the_same_revision() {
        let dir = state_dir("cache-hit");
        let git = FakeGit::in_clone("/repo", ORIGIN);
        let ws = open(&git, &gerrit(), "12345", &dir).unwrap();
        assert!(ws.load_verdicts().is_none(), "nothing cached yet");

        let mut cache = ws.new_verdict_cache();
        cache.put("c1", serde_json::json!({"verdict": "agree"}));
        cache.add_cost(0.14);
        cache.save(&ws.paths.verdicts()).unwrap();

        let loaded = ws.load_verdicts().expect("a hit at the same revision");
        assert_eq!(loaded.get("c1").unwrap()["verdict"], "agree");
        assert!((loaded.total_cost_usd - 0.14).abs() < 1e-9);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_new_patchset_invalidates_the_cached_verdicts() {
        let dir = state_dir("cache-miss");
        let git = FakeGit::in_clone("/repo", ORIGIN);
        let ws = open(&git, &gerrit(), "12345", &dir).unwrap();
        ws.new_verdict_cache().save(&ws.paths.verdicts()).unwrap();
        assert!(ws.load_verdicts().is_some());

        // The same change, now at patchset 4.
        let newer = r#")]}'
{"id":"12345","project":"platform/base","branch":"main","current_revision":"newsha",
 "revisions":{"newsha":{"_number":4,"ref":"refs/changes/45/12345/4"}}}"#;
        std::fs::create_dir_all(ws.worktree()).unwrap();
        let ws2 = open(
            &FakeGit::in_clone("/repo", ORIGIN),
            &FakeGerrit::from_change_json(newer).unwrap(),
            "12345",
            &dir,
        )
        .unwrap();
        assert!(
            ws2.load_verdicts().is_none(),
            "verdicts describing the old patchset must not be reused"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
