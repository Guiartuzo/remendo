//! Finalize: amend the confirmed edits into one patchset, push it, then settle
//! every comment fate in a single call.
//!
//! The sequence is the requirement (specs/change-submission):
//!
//! ```text
//!   re-GET the change
//!      │
//!      ├─ current_revision moved? ──▶ ABORT. Nothing pushed, nothing settled,
//!      │                              both revisions reported.
//!      ▼
//!   stage → amend → push refs/for/<branch>
//!      │
//!      ├─ push rejected? ──▶ surface it, keep the worktree, take NO further
//!      │                     Gerrit action.
//!      ▼
//!   ONE batched review post carrying every fate
//! ```
//!
//! Two of those steps exist because the dry run found their absence. Without the
//! revision check, an amend onto a stale revision pushes a patchset that
//! silently reverts whatever the author uploaded during triage — a wrong result
//! rather than an error. And resolution is not a separate mutation: Gerrit has
//! none, so resolving and replying are one post, which is also why a failed push
//! must settle nothing.

pub mod fate;

pub use fate::{Fate, TriagedThread, build_review};

use crate::gerrit::{GerritApi, GerritError};
use crate::git::{GitCli, GitError};
use crate::workspace::Workspace;

/// What a finalize did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finalized {
    /// The branch the new patchset was pushed to.
    pub branch: String,
    /// Threads resolved by this push.
    pub resolved: usize,
    /// Threads replied to and left unresolved.
    pub replied: usize,
    /// Threads deliberately left untouched.
    pub untouched: usize,
}

/// Amend, push, and settle every comment fate.
///
/// `message` rewrites the commit message and is `Some` only when a
/// `/COMMIT_MSG` comment was accepted — which is why this is not a blanket
/// `--amend --no-edit`. `staged` lists the worktree-relative paths to stage;
/// `git commit --amend` over an unstaged tree amends only the message.
pub fn finalize(
    git: &impl GitCli,
    api: &impl GerritApi,
    workspace: &Workspace,
    triaged: &[TriagedThread],
    staged: &[String],
    message: Option<&str>,
) -> Result<Finalized, SubmitError> {
    let change_id = &workspace.change.id;
    check_revision_unchanged(api, workspace)?;

    let worktree = workspace.worktree();
    for path in staged {
        git.stage(&worktree, path)?;
    }
    git.commit_amend(&worktree, message)?;

    let branch = workspace.change.branch.clone();
    // The worktree is on a detached HEAD, so the push target comes from the
    // change's `branch` field — there is no local branch name to read.
    git.push(&worktree, &format!("HEAD:refs/for/{branch}"))
        .map_err(|source| SubmitError::PushRejected {
            branch: branch.clone(),
            source,
        })?;

    // Only now, with a patchset behind it, may anything be settled.
    api.post_review(change_id, &build_review(triaged))?;
    Ok(summarize(branch, triaged))
}

/// Abort unless the change still points at the revision we checked out.
///
/// Human triage runs for tens of minutes, which is a wide window for the author
/// to upload a new patchset.
fn check_revision_unchanged(
    api: &impl GerritApi,
    workspace: &Workspace,
) -> Result<(), SubmitError> {
    let current = api.change(&workspace.change.id)?;
    let checked_out = &workspace.change.current_revision;
    if current.current_revision != *checked_out {
        return Err(SubmitError::RevisionMoved {
            change_id: workspace.change.id.clone(),
            checked_out: checked_out.clone(),
            current: current.current_revision,
        });
    }
    Ok(())
}

/// Count what the push settled, for the user-facing report.
fn summarize(branch: String, triaged: &[TriagedThread]) -> Finalized {
    let mut summary = Finalized {
        branch,
        resolved: 0,
        replied: 0,
        untouched: 0,
    };
    for item in triaged {
        match &item.fate {
            Fate::Accepted | Fate::FixedByHand => summary.resolved += 1,
            Fate::Rejected { reply: Some(_) } => summary.replied += 1,
            Fate::Rejected { reply: None } | Fate::Skipped => summary.untouched += 1,
        }
    }
    summary
}

/// Failures finalizing a review.
#[derive(Debug, thiserror::Error)]
pub enum SubmitError {
    #[error(
        "change `{change_id}` moved while you were triaging: you reviewed `{checked_out}` \
         but it is now at `{current}`. Nothing was pushed and no comment was settled — \
         a patchset amended onto the older revision would silently revert the newer one."
    )]
    RevisionMoved {
        change_id: String,
        checked_out: String,
        current: String,
    },

    #[error("push to refs/for/{branch} was rejected: {source}. The worktree is left intact.")]
    PushRejected { branch: String, source: GitError },

    #[error(transparent)]
    Git(#[from] GitError),

    #[error(transparent)]
    Gerrit(#[from] GerritError),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gerrit::thread::assemble;
    use crate::gerrit::{ChangeInfo, Comment, FakeGerrit, Thread};
    use crate::git::{FakeGit, GitCall};
    use crate::workspace::SessionPaths;
    use std::path::Path;

    const CHANGE_BODY: &str = r#")]}'
{"id":"12345","project":"proj","branch":"main","current_revision":"sha3",
 "revisions":{"sha3":{"_number":3,"ref":"refs/changes/45/12345/3"}}}"#;

    /// A change at a different revision, as if the author uploaded during triage.
    const MOVED_BODY: &str = r#")]}'
{"id":"12345","project":"proj","branch":"main","current_revision":"sha4",
 "revisions":{"sha4":{"_number":4,"ref":"refs/changes/45/12345/4"}}}"#;

    fn change(body: &str) -> ChangeInfo {
        crate::gerrit::response::decode(body).unwrap()
    }

    fn workspace() -> Workspace {
        Workspace {
            change: change(CHANGE_BODY),
            paths: SessionPaths::new(Path::new("/state"), "proj", "12345").unwrap(),
            resumed: false,
        }
    }

    fn thread(id: &str) -> Thread {
        let comment: Comment = serde_json::from_value(serde_json::json!({
            "id": id, "unresolved": true, "patch_set": 3, "line": 10,
        }))
        .unwrap();
        assemble("src/a.rs", vec![comment]).remove(0)
    }

    fn accepted(id: &str) -> TriagedThread {
        TriagedThread {
            thread: thread(id),
            fate: Fate::Accepted,
        }
    }

    #[test]
    fn a_clean_finalize_stages_amends_pushes_then_posts_once() {
        let git = FakeGit::in_clone("/repo", "u");
        let api = FakeGerrit::from_change_json(CHANGE_BODY).unwrap();
        let ws = workspace();

        let done = finalize(
            &git,
            &api,
            &ws,
            &[accepted("c1")],
            &["src/a.rs".to_string()],
            None,
        )
        .unwrap();

        assert_eq!(done.branch, "main");
        assert_eq!(done.resolved, 1);
        assert_eq!(
            git.calls(),
            vec![
                GitCall::Stage {
                    worktree: ws.worktree(),
                    path: "src/a.rs".into()
                },
                GitCall::CommitAmend {
                    worktree: ws.worktree(),
                    message: None
                },
                GitCall::Push {
                    worktree: ws.worktree(),
                    refspec: "HEAD:refs/for/main".into()
                },
            ]
        );
        assert_eq!(api.posted_reviews().len(), 1, "exactly one batched post");
    }

    /// Dry-run finding #4: without this, an amend onto a stale revision pushes
    /// a patchset that silently reverts the author's newer work.
    #[test]
    fn a_change_that_moved_during_triage_aborts_before_anything_happens() {
        let git = FakeGit::in_clone("/repo", "u");
        let api = FakeGerrit::from_change_json(MOVED_BODY).unwrap();

        let err = finalize(&git, &api, &workspace(), &[accepted("c1")], &[], None).unwrap_err();

        let msg = err.to_string();
        assert!(msg.contains("sha3"), "names what was reviewed: {msg}");
        assert!(msg.contains("sha4"), "names what it is now: {msg}");
        assert!(git.calls().is_empty(), "nothing staged, amended or pushed");
        assert!(api.posted_reviews().is_empty(), "nothing settled");
    }

    /// A rejected push must leave the worktree usable and Gerrit untouched.
    #[test]
    fn a_rejected_push_settles_nothing() {
        struct RejectingGit(FakeGit);
        impl GitCli for RejectingGit {
            fn repo_root(&self) -> Result<std::path::PathBuf, GitError> {
                self.0.repo_root()
            }
            fn remote_url(&self, r: &str) -> Result<String, GitError> {
                self.0.remote_url(r)
            }
            fn config_get(&self, k: &str) -> Result<Option<String>, GitError> {
                self.0.config_get(k)
            }
            fn fill_credential(&self, h: &str) -> Result<crate::git::Credential, GitError> {
                self.0.fill_credential(h)
            }
            fn fetch(&self, r: &str, s: &str) -> Result<(), GitError> {
                self.0.fetch(r, s)
            }
            fn worktree_add(&self, p: &Path, r: &str) -> Result<(), GitError> {
                self.0.worktree_add(p, r)
            }
            fn stage(&self, w: &Path, p: &str) -> Result<(), GitError> {
                self.0.stage(w, p)
            }
            fn commit_amend(&self, w: &Path, m: Option<&str>) -> Result<(), GitError> {
                self.0.commit_amend(w, m)
            }
            fn push(&self, _w: &Path, _r: &str) -> Result<(), GitError> {
                Err(GitError::CommandFailed {
                    command: "push".into(),
                    status: "exit status: 1".into(),
                    stderr: "remote rejected: no new changes".into(),
                })
            }
        }

        let git = RejectingGit(FakeGit::in_clone("/repo", "u"));
        let api = FakeGerrit::from_change_json(CHANGE_BODY).unwrap();
        let err = finalize(&git, &api, &workspace(), &[accepted("c1")], &[], None).unwrap_err();

        assert!(matches!(err, SubmitError::PushRejected { .. }));
        assert!(err.to_string().contains("worktree is left intact"));
        assert!(
            api.posted_reviews().is_empty(),
            "a resolution with no patchset behind it would be a lie"
        );
    }

    /// An accepted /COMMIT_MSG comment rewrites the message, so finalize cannot
    /// be a blanket `--amend --no-edit`.
    #[test]
    fn an_accepted_commit_message_comment_rewrites_the_message() {
        let git = FakeGit::in_clone("/repo", "u");
        let api = FakeGerrit::from_change_json(CHANGE_BODY).unwrap();
        let ws = workspace();

        finalize(
            &git,
            &api,
            &ws,
            &[],
            &[],
            Some("Tighten the parser\n\nBug: 41827\n"),
        )
        .unwrap();

        assert!(git.calls().contains(&GitCall::CommitAmend {
            worktree: ws.worktree(),
            message: Some("Tighten the parser\n\nBug: 41827\n".into()),
        }));
    }

    #[test]
    fn every_confirmed_path_is_staged_before_the_amend() {
        let git = FakeGit::in_clone("/repo", "u");
        let api = FakeGerrit::from_change_json(CHANGE_BODY).unwrap();
        let ws = workspace();

        finalize(
            &git,
            &api,
            &ws,
            &[],
            &["src/a.rs".into(), "src/b.rs".into()],
            None,
        )
        .unwrap();

        let calls = git.calls();
        let amend_at = calls
            .iter()
            .position(|c| matches!(c, GitCall::CommitAmend { .. }))
            .expect("amended");
        let staged: Vec<_> = calls[..amend_at]
            .iter()
            .filter(|c| matches!(c, GitCall::Stage { .. }))
            .collect();
        assert_eq!(staged.len(), 2, "both staged, and before the amend");
    }

    #[test]
    fn the_summary_counts_each_fate() {
        let git = FakeGit::in_clone("/repo", "u");
        let api = FakeGerrit::from_change_json(CHANGE_BODY).unwrap();
        let triaged = vec![
            accepted("c1"),
            TriagedThread {
                thread: thread("c2"),
                fate: Fate::FixedByHand,
            },
            TriagedThread {
                thread: thread("c3"),
                fate: Fate::Rejected {
                    reply: Some("Disagree.".into()),
                },
            },
            TriagedThread {
                thread: thread("c4"),
                fate: Fate::Skipped,
            },
        ];

        let done = finalize(&git, &api, &workspace(), &triaged, &[], None).unwrap();
        assert_eq!(done.resolved, 2);
        assert_eq!(done.replied, 1);
        assert_eq!(done.untouched, 1);
    }

    #[test]
    fn a_finalize_with_nothing_to_settle_still_pushes_the_patchset() {
        let git = FakeGit::in_clone("/repo", "u");
        let api = FakeGerrit::from_change_json(CHANGE_BODY).unwrap();
        let done = finalize(&git, &api, &workspace(), &[], &[], None).unwrap();

        assert_eq!(done.resolved, 0);
        assert!(
            git.calls()
                .iter()
                .any(|c| matches!(c, GitCall::Push { .. }))
        );
        assert_eq!(
            api.posted_reviews().len(),
            1,
            "the post is still issued; it simply carries no comments"
        );
        assert!(api.posted_reviews()[0].comments.is_empty());
    }
}
