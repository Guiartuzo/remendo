//! Applying accepted comments, one scoped turn at a time, behind a confirm gate.
//!
//! One rule governs this module, and it is the fix for the dry run's
//! highest-severity finding (silent data loss):
//!
//! > **The reference is the PRE-TURN SNAPSHOT, never the patchset baseline.**
//!
//! Both the confirm-diff and the revert use the file as it stood *immediately
//! before this turn*. For the first comment in a file the two are the same; for
//! every later one they differ, and the difference is destructive in both
//! directions:
//!
//! ```text
//!   comment 1 confirmed ──▶ file now holds edit 1
//!   comment 2 applied
//!        │
//!        ├─ diff vs PATCHSET  ─▶ shows edit 1 too; the reviewer approves work
//!        │                       they cannot see the boundary of
//!        └─ revert to PATCHSET ─▶ DESTROYS edit 1  (`git checkout --` does this)
//! ```
//!
//! Hence: no `git checkout -- <file>` anywhere in this module, and the snapshot
//! is taken per turn rather than per file.

pub mod files;

pub use files::{FakeWorktree, RealWorktree, WorktreeFiles};

use std::path::{Path, PathBuf};

use crate::diff_view::DiffView;
use crate::driver::{ClaudeDriver, DriverError, SessionId};
use crate::git::{GitCli, GitError};
use crate::submit::{Fate, TriagedThread};

/// What the human decided about one apply turn's edit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfirmChoice {
    /// Keep the edit. It is staged and accumulates in the worktree.
    Keep,
    /// Discard the edit and try again, passing the turn an added hint.
    RetryWithHint(String),
    /// Discard the edit and move on. The comment gets no fix.
    Skip,
}

/// The human at the confirm gate.
///
/// A trait rather than a callback so the UI and the tests implement the same
/// named thing, per `config.yaml`'s no-ad-hoc-stubs rule.
pub trait ConfirmEdits {
    /// Show the confirm-diff and return the decision.
    fn confirm(&mut self, path: &str, diff: &DiffView) -> ConfirmChoice;
}

/// The outcome of applying every accepted comment.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Applied {
    /// Worktree-relative paths with confirmed edits, for finalize to stage.
    pub staged: Vec<String>,
    /// Comments whose edit was confirmed.
    pub kept: usize,
    /// Comments left without a fix.
    pub skipped: usize,
}

/// The accepted threads an apply turn can act on, in order.
///
/// Only real files are editable. An accepted `/COMMIT_MSG` comment routes into
/// the finalize amend's message instead, and `/PATCHSET_LEVEL` never gets an
/// apply turn — neither is a file, so there is nothing on disk to edit.
/// Order within a file is preserved, since later turns read what earlier ones
/// wrote.
pub fn applicable(triaged: &[TriagedThread]) -> Vec<&TriagedThread> {
    triaged
        .iter()
        .filter(|item| item.fate == Fate::Accepted && item.thread.anchor.is_editable_file())
        .collect()
}

/// Apply every accepted comment, confirming each edit before it is kept.
pub fn apply_accepted(
    driver: &impl ClaudeDriver,
    git: &impl GitCli,
    worktree_files: &impl WorktreeFiles,
    worktree_root: &Path,
    session: &SessionId,
    triaged: &[TriagedThread],
    confirm: &mut impl ConfirmEdits,
) -> Result<Applied, ApplyError> {
    let mut applied = Applied::default();
    for item in applicable(triaged) {
        let path = item
            .thread
            .anchor
            .file_path()
            .expect("applicable() kept only file anchors")
            .to_string();
        let kept = apply_one(
            driver,
            git,
            worktree_files,
            worktree_root,
            session,
            &path,
            confirm,
        )?;
        if kept {
            applied.kept += 1;
            if !applied.staged.contains(&path) {
                applied.staged.push(path);
            }
        } else {
            applied.skipped += 1;
        }
    }
    Ok(applied)
}

/// One comment: snapshot, turn, confirm — retrying while the human asks for it.
///
/// Returns whether an edit was kept.
fn apply_one(
    driver: &impl ClaudeDriver,
    git: &impl GitCli,
    worktree_files: &impl WorktreeFiles,
    worktree_root: &Path,
    session: &SessionId,
    path: &str,
    confirm: &mut impl ConfirmEdits,
) -> Result<bool, ApplyError> {
    let mut hint: Option<String> = None;
    loop {
        // Immediately before the turn — not once per file. Everything below
        // measures against this.
        let snapshot = worktree_files.read(path)?;
        let prompt = build_prompt(path, hint.as_deref());
        driver
            .apply_turn(session, &prompt, worktree_root)?
            .payload("apply")?;

        let edited = worktree_files.read(path)?;
        let diff = DiffView::new(path, &snapshot, &edited);
        match confirm.confirm(path, &diff) {
            ConfirmChoice::Keep => {
                git.stage(worktree_root, path)?;
                return Ok(true);
            }
            // Restore the SNAPSHOT, never the patchset baseline: `git checkout
            // -- <path>` here would destroy edits confirmed for earlier
            // comments on this same file.
            ConfirmChoice::RetryWithHint(text) => {
                worktree_files.write(path, &snapshot)?;
                hint = Some(text);
            }
            ConfirmChoice::Skip => {
                worktree_files.write(path, &snapshot)?;
                return Ok(false);
            }
        }
    }
}

/// The apply turn's instruction.
///
/// Prompt *prose* is deliberately unfinished — `tasks.md` 9.2 owns the wording,
/// and design.md §7 owns the contract. What matters here is the shape: the
/// comment is identified by its quoted code rather than its line number,
/// because anchors drift as earlier edits land.
fn build_prompt(path: &str, hint: Option<&str>) -> String {
    let mut prompt = format!("Re-read {path}, then address the quoted comment there.");
    if let Some(hint) = hint {
        prompt.push_str("\n\nThe previous attempt was rejected: ");
        prompt.push_str(hint);
    }
    prompt
}

/// Failures applying an accepted comment.
#[derive(Debug, thiserror::Error)]
pub enum ApplyError {
    #[error(transparent)]
    Driver(#[from] DriverError),

    #[error(transparent)]
    Git(#[from] GitError),

    #[error("could not access {path}: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::driver::FakeDriver;
    use crate::gerrit::thread::assemble;
    use crate::gerrit::{Comment, Thread};
    use crate::git::{FakeGit, GitCall};

    fn thread_on(path: &str, id: &str) -> Thread {
        let comment: Comment = serde_json::from_value(serde_json::json!({
            "id": id, "unresolved": true, "patch_set": 3, "line": 1,
        }))
        .unwrap();
        assemble(path, vec![comment]).remove(0)
    }

    fn accepted(path: &str, id: &str) -> TriagedThread {
        TriagedThread {
            thread: thread_on(path, id),
            fate: Fate::Accepted,
        }
    }

    /// A confirm gate that answers from a script, recording what it was shown.
    struct ScriptedConfirm {
        answers: Vec<ConfirmChoice>,
        seen: Vec<(String, Vec<String>)>,
    }

    impl ScriptedConfirm {
        fn new(answers: Vec<ConfirmChoice>) -> Self {
            Self {
                answers,
                seen: Vec::new(),
            }
        }
    }

    impl ConfirmEdits for ScriptedConfirm {
        fn confirm(&mut self, path: &str, diff: &DiffView) -> ConfirmChoice {
            self.seen.push((path.to_string(), diff.changed_lines()));
            if self.answers.is_empty() {
                ConfirmChoice::Skip
            } else {
                self.answers.remove(0)
            }
        }
    }

    fn session() -> SessionId {
        SessionId("test-session".into())
    }

    #[test]
    fn a_confirmed_edit_is_kept_and_staged() {
        let wt = FakeWorktree::with_files(&[("a.rs", "fn a() {}\n")]);
        let driver = FakeDriver::new(wt.clone()).editing("a.rs", "fn a() { one(); }\n");
        let git = FakeGit::in_clone("/repo", "u");
        let mut confirm = ScriptedConfirm::new(vec![ConfirmChoice::Keep]);

        let applied = apply_accepted(
            &driver,
            &git,
            &wt,
            Path::new("/wt"),
            &session(),
            &[accepted("a.rs", "c1")],
            &mut confirm,
        )
        .unwrap();

        assert_eq!(applied.kept, 1);
        assert_eq!(applied.staged, vec!["a.rs".to_string()]);
        assert_eq!(wt.contents("a.rs").unwrap(), "fn a() { one(); }\n");
        assert!(git.calls().contains(&GitCall::Stage {
            worktree: "/wt".into(),
            path: "a.rs".into()
        }));
    }

    #[test]
    fn a_skipped_edit_is_reverted_and_not_staged() {
        let wt = FakeWorktree::with_files(&[("a.rs", "original\n")]);
        let driver = FakeDriver::new(wt.clone()).editing("a.rs", "unwanted\n");
        let git = FakeGit::in_clone("/repo", "u");
        let mut confirm = ScriptedConfirm::new(vec![ConfirmChoice::Skip]);

        let applied = apply_accepted(
            &driver,
            &git,
            &wt,
            Path::new("/wt"),
            &session(),
            &[accepted("a.rs", "c1")],
            &mut confirm,
        )
        .unwrap();

        assert_eq!(applied.skipped, 1);
        assert!(applied.staged.is_empty());
        assert_eq!(wt.contents("a.rs").unwrap(), "original\n");
        assert!(git.calls().is_empty(), "nothing staged");
    }

    /// **Dry-run finding #1, the highest-severity one.** Reject used to restore
    /// the patchset baseline, which destroyed edits already confirmed for
    /// earlier comments on the same file.
    #[test]
    fn rejecting_comment_two_preserves_comment_ones_confirmed_edit() {
        let baseline = "fn a() {}\nfn b() {}\n";
        let after_one = "fn a() { one(); }\nfn b() {}\n";
        let after_two = "fn a() { one(); }\nfn b() { two(); }\n";

        let wt = FakeWorktree::with_files(&[("a.rs", baseline)]);
        let driver = FakeDriver::new(wt.clone())
            .editing("a.rs", after_one)
            .editing("a.rs", after_two);
        let git = FakeGit::in_clone("/repo", "u");
        let mut confirm = ScriptedConfirm::new(vec![ConfirmChoice::Keep, ConfirmChoice::Skip]);

        let applied = apply_accepted(
            &driver,
            &git,
            &wt,
            Path::new("/wt"),
            &session(),
            &[accepted("a.rs", "c1"), accepted("a.rs", "c2")],
            &mut confirm,
        )
        .unwrap();

        assert_eq!(applied.kept, 1);
        assert_eq!(applied.skipped, 1);
        assert_eq!(
            wt.contents("a.rs").unwrap(),
            after_one,
            "comment 1's confirmed edit must survive comment 2's rejection"
        );
        assert!(
            wt.contents("a.rs").unwrap().contains("one();"),
            "the destroyed-work regression this test exists for"
        );
    }

    /// The confirm-diff shows only what THIS turn changed. Diffed against the
    /// patchset, comment 2's diff would contain comment 1's approved work.
    #[test]
    fn the_confirm_diff_shows_only_this_turns_change() {
        let wt = FakeWorktree::with_files(&[("a.rs", "fn a() {}\nfn b() {}\n")]);
        let driver = FakeDriver::new(wt.clone())
            .editing("a.rs", "fn a() { one(); }\nfn b() {}\n")
            .editing("a.rs", "fn a() { one(); }\nfn b() { two(); }\n");
        let git = FakeGit::in_clone("/repo", "u");
        let mut confirm = ScriptedConfirm::new(vec![ConfirmChoice::Keep, ConfirmChoice::Keep]);

        apply_accepted(
            &driver,
            &git,
            &wt,
            Path::new("/wt"),
            &session(),
            &[accepted("a.rs", "c1"), accepted("a.rs", "c2")],
            &mut confirm,
        )
        .unwrap();

        let second_diff = &confirm.seen[1].1;
        assert!(
            second_diff.iter().all(|line| line.contains("fn b()")),
            "comment 2's diff leaked comment 1's work: {second_diff:?}"
        );
    }

    #[test]
    fn a_retry_reverts_then_runs_again_with_the_hint() {
        let wt = FakeWorktree::with_files(&[("a.rs", "original\n")]);
        let driver = FakeDriver::new(wt.clone())
            .editing("a.rs", "first attempt\n")
            .editing("a.rs", "second attempt\n");
        let git = FakeGit::in_clone("/repo", "u");
        let mut confirm = ScriptedConfirm::new(vec![
            ConfirmChoice::RetryWithHint("too broad".into()),
            ConfirmChoice::Keep,
        ]);

        let applied = apply_accepted(
            &driver,
            &git,
            &wt,
            Path::new("/wt"),
            &session(),
            &[accepted("a.rs", "c1")],
            &mut confirm,
        )
        .unwrap();

        assert_eq!(applied.kept, 1);
        assert_eq!(wt.contents("a.rs").unwrap(), "second attempt\n");
        assert_eq!(driver.turns().len(), 2);
        assert!(
            driver.turns()[1].contains("too broad"),
            "the hint reaches the retry: {:?}",
            driver.turns()[1]
        );
        // The retry measured against the ORIGINAL, not the rejected attempt.
        assert!(
            !driver.turns()[1].contains("first attempt"),
            "the rejected attempt was reverted before retrying"
        );
    }

    #[test]
    fn pseudo_path_comments_never_get_an_apply_turn() {
        let triaged = vec![
            accepted("/COMMIT_MSG", "m1"),
            accepted("/PATCHSET_LEVEL", "p1"),
            accepted("a.rs", "c1"),
        ];
        let selected = applicable(&triaged);
        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].thread.anchor.file_path(), Some("a.rs"));
    }

    #[test]
    fn only_accepted_threads_are_applied() {
        let triaged = vec![
            accepted("a.rs", "c1"),
            TriagedThread {
                thread: thread_on("a.rs", "c2"),
                fate: Fate::Skipped,
            },
            TriagedThread {
                thread: thread_on("a.rs", "c3"),
                fate: Fate::FixedByHand,
            },
            TriagedThread {
                thread: thread_on("a.rs", "c4"),
                fate: Fate::Rejected { reply: None },
            },
        ];
        assert_eq!(applicable(&triaged).len(), 1);
    }

    #[test]
    fn a_file_touched_by_two_comments_is_staged_once() {
        let wt = FakeWorktree::with_files(&[("a.rs", "0\n")]);
        let driver = FakeDriver::new(wt.clone())
            .editing("a.rs", "1\n")
            .editing("a.rs", "2\n");
        let git = FakeGit::in_clone("/repo", "u");
        let mut confirm = ScriptedConfirm::new(vec![ConfirmChoice::Keep, ConfirmChoice::Keep]);

        let applied = apply_accepted(
            &driver,
            &git,
            &wt,
            Path::new("/wt"),
            &session(),
            &[accepted("a.rs", "c1"), accepted("a.rs", "c2")],
            &mut confirm,
        )
        .unwrap();

        assert_eq!(applied.kept, 2);
        assert_eq!(applied.staged, vec!["a.rs".to_string()], "one path entry");
    }

    #[test]
    fn a_failed_turn_stops_rather_than_confirming_nothing() {
        let wt = FakeWorktree::with_files(&[("a.rs", "original\n")]);
        let driver = FakeDriver::new(wt.clone()).failing();
        let git = FakeGit::in_clone("/repo", "u");
        let mut confirm = ScriptedConfirm::new(vec![ConfirmChoice::Keep]);

        let err = apply_accepted(
            &driver,
            &git,
            &wt,
            Path::new("/wt"),
            &session(),
            &[accepted("a.rs", "c1")],
            &mut confirm,
        )
        .unwrap_err();

        assert!(matches!(err, ApplyError::Driver(_)));
        assert!(confirm.seen.is_empty(), "no diff shown for a failed turn");
        assert_eq!(wt.contents("a.rs").unwrap(), "original\n");
    }
}
