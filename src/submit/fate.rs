//! What triage decided about a thread, and how that becomes a review post.
//!
//! This is finalize's **input contract**: triage (§5) produces it, finalize
//! consumes it. It lives here rather than with the UI because the shape is
//! determined by what Gerrit can be told, not by how a human chose it.
//!
//! Gerrit exposes no mark-comment-resolved mutation — a thread is resolved by
//! replying to it with `unresolved: false` — so resolution and reply are the
//! same operation differing only in that flag, and every fate below is either
//! one such comment or nothing at all.

use crate::gerrit::{ReviewComment, ReviewInput, Thread};

/// The message posted when a thread is resolved.
///
/// Gerrit requires text on a comment, and resolving *is* commenting, so silence
/// is not an option the API offers. Deliberately identical for applied and
/// hand-written fixes: the spec requires those to be indistinguishable at this
/// layer, and a differing message would leak the distinction back out.
const RESOLVED_MESSAGE: &str = "Done.";

/// What the human decided about a thread.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Fate {
    /// An apply turn produced a fix that the user confirmed.
    Accepted,
    /// The user fixed it themselves in their own editor. Resolves exactly as
    /// `Accepted` does — the spec requires them to be indistinguishable here.
    FixedByHand,
    /// The user rejected the comment. `reply` is the text approved during
    /// triage; `None` means the drafted reply was declined, and a declined
    /// draft posts nothing at all.
    Rejected { reply: Option<String> },
    /// Undecided at the triage gate, or explicitly passed over. Neither
    /// resolved nor replied to.
    Skipped,
}

impl Fate {
    /// Whether this fate resolves the thread.
    pub fn resolves(&self) -> bool {
        matches!(self, Fate::Accepted | Fate::FixedByHand)
    }
}

/// A thread together with what triage decided about it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TriagedThread {
    pub thread: Thread,
    pub fate: Fate,
}

impl TriagedThread {
    /// The review comment this fate posts, or `None` when it posts nothing.
    fn review_comment(&self) -> Option<ReviewComment> {
        let (message, unresolved) = match &self.fate {
            Fate::Accepted | Fate::FixedByHand => (RESOLVED_MESSAGE.to_string(), false),
            // A rejection stays unresolved: the disagreement is not settled by
            // stating it.
            Fate::Rejected { reply: Some(text) } => (text.clone(), true),
            Fate::Rejected { reply: None } | Fate::Skipped => return None,
        };
        Some(ReviewComment {
            path: self.thread.anchor.gerrit_path().to_string(),
            line: self.thread.line(),
            // Replies attach to the END of the exchange, not its opening
            // comment (design.md §13).
            in_reply_to: self.thread.reply_target().to_string(),
            message,
            unresolved,
        })
    }
}

/// Build the single batched review post carrying every thread's fate.
///
/// Issued only after a successful push — a resolution with no patchset behind
/// it would be a lie.
pub fn build_review(triaged: &[TriagedThread]) -> ReviewInput {
    let mut review = ReviewInput::default();
    for item in triaged {
        if let Some(comment) = item.review_comment() {
            review
                .comments
                .entry(comment.path.clone())
                .or_default()
                .push(comment);
        }
    }
    review
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gerrit::Comment;
    use crate::gerrit::thread::assemble;

    /// A single-comment thread on `path`, anchored at line 10.
    fn thread_on(path: &str, id: &str) -> Thread {
        let comment: Comment = serde_json::from_value(serde_json::json!({
            "id": id, "unresolved": true, "patch_set": 3, "line": 10,
        }))
        .unwrap();
        assemble(path, vec![comment]).remove(0)
    }

    /// A two-comment thread, so the reply target is not the opening comment.
    fn exchange_on(path: &str) -> Thread {
        let comments: Vec<Comment> = serde_json::from_value(serde_json::json!([
            {"id": "first", "unresolved": true, "patch_set": 3, "line": 10,
             "updated": "2026-08-01 10:00"},
            {"id": "last", "in_reply_to": "first", "unresolved": true, "patch_set": 3,
             "updated": "2026-08-01 11:00"}
        ]))
        .unwrap();
        assemble(path, comments).remove(0)
    }

    fn triaged(thread: Thread, fate: Fate) -> TriagedThread {
        TriagedThread { thread, fate }
    }

    #[test]
    fn an_accepted_thread_is_resolved() {
        let review = build_review(&[triaged(thread_on("a.rs", "c1"), Fate::Accepted)]);
        let comment = &review.comments["a.rs"][0];
        assert!(!comment.unresolved);
        assert_eq!(comment.line, Some(10));
    }

    /// The spec requires these to be indistinguishable at this layer, so the
    /// posted comments must be byte-identical.
    #[test]
    fn a_hand_fixed_thread_is_indistinguishable_from_an_accepted_one() {
        let applied = build_review(&[triaged(thread_on("a.rs", "c1"), Fate::Accepted)]);
        let by_hand = build_review(&[triaged(thread_on("a.rs", "c1"), Fate::FixedByHand)]);
        assert_eq!(applied, by_hand);
    }

    #[test]
    fn a_rejected_thread_posts_its_reply_and_stays_unresolved() {
        let review = build_review(&[triaged(
            thread_on("a.rs", "c1"),
            Fate::Rejected {
                reply: Some("Bounded at 8 items, so O(n²) is not a concern here.".into()),
            },
        )]);
        let comment = &review.comments["a.rs"][0];
        assert!(
            comment.unresolved,
            "stating a disagreement does not settle it"
        );
        assert!(comment.message.contains("Bounded at 8"));
    }

    #[test]
    fn a_declined_draft_posts_nothing() {
        let review = build_review(&[triaged(
            thread_on("a.rs", "c1"),
            Fate::Rejected { reply: None },
        )]);
        assert!(review.comments.is_empty());
    }

    #[test]
    fn skipped_threads_are_omitted_entirely() {
        let review = build_review(&[triaged(thread_on("a.rs", "c1"), Fate::Skipped)]);
        assert!(
            review.comments.is_empty(),
            "a skipped thread is neither resolved nor replied to"
        );
    }

    /// Replies attach to the end of the exchange; targeting the opening comment
    /// would answer a question the thread already moved past.
    #[test]
    fn a_reply_targets_the_threads_last_comment() {
        let review = build_review(&[triaged(
            exchange_on("a.rs"),
            Fate::Rejected {
                reply: Some("Still disagree.".into()),
            },
        )]);
        assert_eq!(review.comments["a.rs"][0].in_reply_to, "last");
    }

    #[test]
    fn several_threads_on_one_file_share_a_path_entry() {
        let review = build_review(&[
            triaged(thread_on("a.rs", "c1"), Fate::Accepted),
            triaged(thread_on("a.rs", "c2"), Fate::Accepted),
        ]);
        assert_eq!(review.comments["a.rs"].len(), 2);
        assert_eq!(review.comments.len(), 1, "one entry per path");
    }

    #[test]
    fn a_commit_message_thread_posts_under_its_pseudo_path() {
        let review = build_review(&[triaged(thread_on("/COMMIT_MSG", "m1"), Fate::Accepted)]);
        assert!(review.comments.contains_key("/COMMIT_MSG"));
    }

    #[test]
    fn a_change_level_thread_posts_without_a_line() {
        let comment: Comment =
            serde_json::from_value(serde_json::json!({"id": "p1", "unresolved": true})).unwrap();
        let thread = assemble("/PATCHSET_LEVEL", vec![comment]).remove(0);
        let review = build_review(&[triaged(
            thread,
            Fate::Rejected {
                reply: Some("No.".into()),
            },
        )]);
        let posted = &review.comments["/PATCHSET_LEVEL"][0];
        assert_eq!(posted.line, None);
        // Serialization drops it entirely rather than sending null.
        let json = serde_json::to_value(posted).unwrap();
        assert!(json.get("line").is_none());
    }

    #[test]
    fn a_mixed_triage_carries_only_the_fates_that_post() {
        let review = build_review(&[
            triaged(thread_on("a.rs", "c1"), Fate::Accepted),
            triaged(thread_on("a.rs", "c2"), Fate::Skipped),
            triaged(thread_on("b.rs", "c3"), Fate::FixedByHand),
            triaged(
                thread_on("b.rs", "c4"),
                Fate::Rejected {
                    reply: Some("Disagree.".into()),
                },
            ),
            triaged(thread_on("b.rs", "c5"), Fate::Rejected { reply: None }),
        ]);
        assert_eq!(review.comments["a.rs"].len(), 1, "skipped is dropped");
        assert_eq!(
            review.comments["b.rs"].len(),
            2,
            "declined draft is dropped"
        );
        let resolved = review.comments["b.rs"]
            .iter()
            .filter(|c| !c.unresolved)
            .count();
        assert_eq!(resolved, 1);
    }

    #[test]
    fn an_empty_triage_produces_an_empty_review() {
        assert!(build_review(&[]).comments.is_empty());
    }

    #[test]
    fn resolves_reports_which_fates_settle_a_thread() {
        assert!(Fate::Accepted.resolves());
        assert!(Fate::FixedByHand.resolves());
        assert!(!Fate::Rejected { reply: None }.resolves());
        assert!(!Fate::Skipped.resolves());
    }
}
