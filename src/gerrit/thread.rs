//! Assembling Gerrit's flat comment list into threads, and filtering them.
//!
//! This is the module `open-decisions.md` called the one most likely to ship a
//! subtly broken tool, so its rules are stated rather than implied:
//!
//! * A thread's state is its **last** comment's `unresolved` flag. Filtering
//!   per comment re-opens threads that were explicitly settled.
//! * The **first** comment supplies the anchor, the **whole exchange** is the
//!   question, and the **last** comment is the reply target.
//! * Ordering is by `updated`, so a branching thread's "last" comment is the
//!   most recent one — which is also the tiebreak design.md §13 specifies.

use std::collections::HashMap;

use super::anchor::CommentAnchor;
use super::comment::Comment;

/// An assembled comment thread: every comment sharing a root, in order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Thread {
    /// What the thread hangs off, taken from its first comment.
    pub anchor: CommentAnchor,
    /// The exchange, ordered oldest first. Never empty.
    pub comments: Vec<Comment>,
}

impl Thread {
    /// The comment that opened the thread — the source of its anchor and line.
    pub fn root(&self) -> &Comment {
        self.comments.first().expect("a thread is never empty")
    }

    /// The most recent comment — the source of the thread's state and the
    /// target of any reply.
    pub fn last(&self) -> &Comment {
        self.comments.last().expect("a thread is never empty")
    }

    /// Whether the thread still needs attention.
    ///
    /// Reads the **last** comment's flag. A reviewer's `unresolved: true`
    /// concern followed by a reply carrying `unresolved: false` is closed, and
    /// the opening comment's flag says otherwise.
    pub fn is_unresolved(&self) -> bool {
        self.last().unresolved
    }

    /// The comment id a reply must set `in_reply_to` to.
    pub fn reply_target(&self) -> &str {
        &self.last().id
    }

    /// The line the thread is anchored at, from its opening comment.
    pub fn line(&self) -> Option<u32> {
        self.root().line
    }

    /// The patchset the thread was opened against.
    pub fn patch_set(&self) -> u32 {
        self.root().patch_set
    }

    /// Whether a linter or CI job opened this thread.
    pub fn is_robot(&self) -> bool {
        self.root().is_robot()
    }

    /// The account that opened the thread, for provenance filtering. Thread
    /// level deliberately: excluding individual comments the user wrote would
    /// drop their reply out of a reviewer's thread and take the thread's true
    /// state with it.
    pub fn started_by(&self) -> Option<u64> {
        self.root().author.account_id
    }
}

/// The threads of one change, plus what was left out of them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ThreadSet {
    /// Threads to triage.
    pub threads: Vec<Thread>,
    /// Unresolved threads dropped because they are anchored on an earlier
    /// patchset. Carried as a count so the omission is reportable — a shorter
    /// queue must never be indistinguishable from a change with fewer comments.
    pub skipped_older_patchsets: usize,
}

/// Assemble a file's flat comment list into threads.
///
/// `path` is the Gerrit comment path, classified once into a [`CommentAnchor`]
/// so no pseudo-path escapes as a filesystem path.
///
/// ```
/// # use remendo::gerrit::{thread::assemble, Comment};
/// let comments: Vec<Comment> = serde_json::from_str(r#"[
///     {"id":"a","updated":"2026-08-01 10:00:00.000000000","unresolved":true},
///     {"id":"b","in_reply_to":"a","updated":"2026-08-01 11:00:00.000000000"}
/// ]"#).unwrap();
/// let threads = assemble("src/a.rs", comments);
/// assert_eq!(threads.len(), 1);
/// // Closed by the reply, even though the opening comment says unresolved.
/// assert!(!threads[0].is_unresolved());
/// ```
pub fn assemble(path: &str, comments: Vec<Comment>) -> Vec<Thread> {
    let anchor = CommentAnchor::classify(path);
    let parents: HashMap<String, Option<String>> = comments
        .iter()
        .map(|c| (c.id.clone(), c.in_reply_to.clone()))
        .collect();

    let mut by_root: HashMap<String, Vec<Comment>> = HashMap::new();
    for comment in comments {
        let root = root_of(&comment.id, &parents);
        by_root.entry(root).or_default().push(comment);
    }

    let mut threads: Vec<Thread> = by_root
        .into_values()
        .map(|mut comments| {
            comments.sort_by(|a, b| a.updated.cmp(&b.updated).then_with(|| a.id.cmp(&b.id)));
            Thread {
                anchor: anchor.clone(),
                comments,
            }
        })
        .collect();

    // Stable output so the triage queue does not reshuffle between runs;
    // HashMap iteration order is not.
    threads.sort_by(|a, b| {
        a.line()
            .cmp(&b.line())
            .then_with(|| a.root().updated.cmp(&b.root().updated))
            .then_with(|| a.root().id.cmp(&b.root().id))
    });
    threads
}

/// Walk `in_reply_to` links up to the thread's root id.
///
/// A comment whose parent is not in the set is treated as its own root — Gerrit
/// can return a reply whose parent lives under another path, and dropping it
/// would lose a live thread.
///
/// A cyclic chain is malformed data Gerrit should never produce, but merely
/// *terminating* on one is not enough: each member of the cycle would stop at a
/// different id and the same conversation would appear as several threads, to
/// be triaged more than once. Collapsing onto the smallest id in the cycle makes
/// every member agree on one root.
fn root_of(id: &str, parents: &HashMap<String, Option<String>>) -> String {
    let mut current = id;
    let mut seen = vec![current];
    while let Some(Some(parent)) = parents.get(current) {
        let parent = parent.as_str();
        if !parents.contains_key(parent) {
            break;
        }
        if seen.contains(&parent) {
            return seen.iter().min().expect("seen is never empty").to_string();
        }
        current = parent;
        seen.push(current);
    }
    current.to_string()
}

/// Keep the threads that should be triaged, counting what patchset age drops.
///
/// Retains unresolved threads anchored on `current_patch_set`. Drafts never
/// reach here — they are a separate endpoint that is not fetched — and robot
/// threads and the user's own threads are deliberately kept (design.md §13).
///
/// ```
/// # use remendo::gerrit::thread::{assemble, select_triagable};
/// # use remendo::gerrit::Comment;
/// let comments: Vec<Comment> = serde_json::from_str(r#"[
///     {"id":"old","unresolved":true,"patch_set":1},
///     {"id":"new","unresolved":true,"patch_set":3}
/// ]"#).unwrap();
/// let set = select_triagable(assemble("a.rs", comments), 3);
/// assert_eq!(set.threads.len(), 1);
/// assert_eq!(set.skipped_older_patchsets, 1);
/// ```
pub fn select_triagable(threads: Vec<Thread>, current_patch_set: u32) -> ThreadSet {
    let mut set = ThreadSet::default();
    for thread in threads {
        if !thread.is_unresolved() {
            continue;
        }
        if thread.patch_set() < current_patch_set {
            set.skipped_older_patchsets += 1;
            continue;
        }
        set.threads.push(thread);
    }
    set
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a comment from the fields these tests care about.
    fn comment(id: &str, reply_to: Option<&str>, updated: &str, unresolved: bool) -> Comment {
        serde_json::from_value(serde_json::json!({
            "id": id,
            "in_reply_to": reply_to,
            "updated": updated,
            "unresolved": unresolved,
            "patch_set": 1,
            "line": 10,
        }))
        .expect("fixture parses")
    }

    // --- thread state ------------------------------------------------------

    /// The case open-decisions.md led with: a concern answered by a reply that
    /// closed it. Filtering per comment would re-surface the opening comment.
    #[test]
    fn a_thread_closed_by_its_reply_is_resolved() {
        let threads = assemble(
            "a.rs",
            vec![
                comment("a", None, "2026-08-01 10:00", true),
                comment("b", Some("a"), "2026-08-01 11:00", false),
            ],
        );
        assert_eq!(threads.len(), 1);
        assert!(!threads[0].is_unresolved());
        assert!(
            threads[0].comments[0].unresolved,
            "the opening comment still says unresolved — which is why per-comment \
             filtering is wrong"
        );
    }

    #[test]
    fn a_thread_reopened_by_a_later_comment_is_unresolved() {
        let threads = assemble(
            "a.rs",
            vec![
                comment("a", None, "2026-08-01 10:00", true),
                comment("b", Some("a"), "2026-08-01 11:00", false),
                comment("c", Some("b"), "2026-08-01 12:00", true),
            ],
        );
        assert!(threads[0].is_unresolved());
    }

    // --- the live ask is the last comment ----------------------------------

    /// design.md §13's worked example: the reviewer conceded the original point
    /// and narrowed the ask. The whole exchange must reach the verdict turn.
    #[test]
    fn the_whole_exchange_is_kept_in_order() {
        let threads = assemble(
            "a.rs",
            vec![
                comment("c", Some("b"), "2026-08-01 12:00", true),
                comment("a", None, "2026-08-01 10:00", true),
                comment("b", Some("a"), "2026-08-01 11:00", true),
            ],
        );
        let ids: Vec<&str> = threads[0].comments.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(ids, vec!["a", "b", "c"], "ordered oldest first");
        assert_eq!(threads[0].root().id, "a", "anchor comes from the first");
        assert_eq!(threads[0].reply_target(), "c", "reply targets the last");
    }

    #[test]
    fn a_branching_thread_resolves_by_recency() {
        // Two replies to the same parent: the later one is the thread's last.
        let threads = assemble(
            "a.rs",
            vec![
                comment("a", None, "2026-08-01 10:00", true),
                comment("b", Some("a"), "2026-08-01 11:00", true),
                comment("c", Some("a"), "2026-08-01 12:00", false),
            ],
        );
        assert_eq!(threads.len(), 1, "one thread, not two");
        assert_eq!(threads[0].reply_target(), "c");
        assert!(!threads[0].is_unresolved(), "state is the most recent flag");
    }

    // --- assembly edge cases -----------------------------------------------

    #[test]
    fn independent_roots_become_separate_threads() {
        let threads = assemble(
            "a.rs",
            vec![
                comment("a", None, "2026-08-01 10:00", true),
                comment("b", None, "2026-08-01 11:00", true),
            ],
        );
        assert_eq!(threads.len(), 2);
    }

    #[test]
    fn a_reply_whose_parent_is_absent_becomes_its_own_thread() {
        // Gerrit can return a reply whose parent sits under another path;
        // dropping it would lose a live thread.
        let threads = assemble(
            "a.rs",
            vec![comment(
                "orphan",
                Some("elsewhere"),
                "2026-08-01 10:00",
                true,
            )],
        );
        assert_eq!(threads.len(), 1);
        assert_eq!(threads[0].root().id, "orphan");
    }

    #[test]
    fn a_cyclic_chain_terminates() {
        let threads = assemble(
            "a.rs",
            vec![
                comment("a", Some("b"), "2026-08-01 10:00", true),
                comment("b", Some("a"), "2026-08-01 11:00", true),
            ],
        );
        assert_eq!(threads.len(), 1, "the cycle collapses to one thread");
    }

    #[test]
    fn assembly_is_stable_across_input_order() {
        let forward = assemble(
            "a.rs",
            vec![
                comment("a", None, "2026-08-01 10:00", true),
                comment("b", None, "2026-08-01 11:00", true),
            ],
        );
        let reversed = assemble(
            "a.rs",
            vec![
                comment("b", None, "2026-08-01 11:00", true),
                comment("a", None, "2026-08-01 10:00", true),
            ],
        );
        assert_eq!(forward, reversed, "the triage queue must not reshuffle");
    }

    #[test]
    fn the_anchor_is_classified_once_for_the_thread() {
        let threads = assemble("/COMMIT_MSG", vec![comment("a", None, "t", true)]);
        assert_eq!(threads[0].anchor, CommentAnchor::CommitMessage);
        assert_eq!(threads[0].anchor.file_path(), None);
    }

    // --- selection ---------------------------------------------------------

    fn at_patch_set(id: &str, patch_set: u32, unresolved: bool) -> Comment {
        serde_json::from_value(serde_json::json!({
            "id": id, "updated": "t", "unresolved": unresolved, "patch_set": patch_set,
        }))
        .expect("fixture parses")
    }

    #[test]
    fn resolved_threads_are_not_triaged_and_not_counted_as_skipped() {
        let threads = assemble("a.rs", vec![at_patch_set("a", 3, false)]);
        let set = select_triagable(threads, 3);
        assert!(set.threads.is_empty());
        assert_eq!(set.skipped_older_patchsets, 0, "resolved is not 'skipped'");
    }

    #[test]
    fn older_patchset_threads_are_skipped_and_counted() {
        let threads = assemble(
            "a.rs",
            vec![at_patch_set("old", 1, true), at_patch_set("cur", 3, true)],
        );
        let set = select_triagable(threads, 3);
        assert_eq!(set.threads.len(), 1);
        assert_eq!(set.threads[0].root().id, "cur");
        assert_eq!(set.skipped_older_patchsets, 1);
    }

    #[test]
    fn a_resolved_older_thread_is_not_double_counted() {
        let threads = assemble("a.rs", vec![at_patch_set("old", 1, false)]);
        let set = select_triagable(threads, 3);
        assert_eq!(
            set.skipped_older_patchsets, 0,
            "an already-settled old thread is not an omission worth reporting"
        );
    }

    #[test]
    fn robot_and_self_authored_threads_are_kept() {
        let robot: Comment = serde_json::from_value(serde_json::json!({
            "id": "r", "robot_id": "clippy", "unresolved": true, "patch_set": 2,
        }))
        .unwrap();
        let mine: Comment = serde_json::from_value(serde_json::json!({
            "id": "m", "author": {"_account_id": 7}, "unresolved": true, "patch_set": 2,
        }))
        .unwrap();
        let set = select_triagable(assemble("a.rs", vec![robot, mine]), 2);
        assert_eq!(set.threads.len(), 2, "both are in scope in v0");
        assert!(set.threads.iter().any(|t| t.is_robot()));
        assert!(set.threads.iter().any(|t| t.started_by() == Some(7)));
    }
}
