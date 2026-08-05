//! The review map: which documents carry threads, and how far triage has got.
//!
//! DIVERGENCE from vybim's `file_tree.rs`, which this does **not** use.
//! That widget walks a directory with `ignore` and answers "what files exist".
//! The triage tree answers a different question — "where is attention still
//! needed" — over a set that includes `/COMMIT_MSG` and `/PATCHSET_LEVEL`,
//! which are not files and would never appear in a directory walk
//! (specs/comment-triage). Reusing it would have meant filtering a filesystem
//! tree down to a list it cannot represent.
//!
//! The annotation model lives here, separate from the drawing, so the counts
//! and the progress are testable.

use crate::gerrit::CommentAnchor;
use crate::triage::Triage;

/// One line of the tree: a document and its triage progress.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeEntry {
    /// The Gerrit path — a real file path, or one of the two pseudo-paths.
    pub path: String,
    /// Whether this is a real file. The pseudo-paths are marked so the tree can
    /// show that they are not on disk.
    pub is_file: bool,
    /// Threads anchored here.
    pub total: usize,
    /// How many of them carry a decision.
    pub decided: usize,
}

impl TreeEntry {
    /// Whether every thread here has been decided.
    pub fn is_complete(&self) -> bool {
        self.decided == self.total
    }

    /// The progress annotation, e.g. `2/3`.
    pub fn progress(&self) -> String {
        format!("{}/{}", self.decided, self.total)
    }

    /// A short marker distinguishing a pseudo-path from a file.
    pub fn kind_marker(&self) -> &'static str {
        if self.is_file { " " } else { "*" }
    }
}

/// Build the tree from the triage queue, in first-seen order.
///
/// Order follows the queue rather than sorting, so the tree reads in the same
/// order the human will meet the threads.
pub fn entries(triage: &Triage) -> Vec<TreeEntry> {
    let mut entries: Vec<TreeEntry> = Vec::new();
    for item in triage.items() {
        let path = item.thread.anchor.gerrit_path().to_string();
        let decided = usize::from(!item.is_undecided());
        match entries.iter_mut().find(|entry| entry.path == path) {
            Some(entry) => {
                entry.total += 1;
                entry.decided += decided;
            }
            None => entries.push(TreeEntry {
                is_file: matches!(item.thread.anchor, CommentAnchor::File(_)),
                path,
                total: 1,
                decided,
            }),
        }
    }
    entries
}

/// The index of the entry holding the thread under the cursor.
pub fn selected(triage: &Triage, entries: &[TreeEntry]) -> Option<usize> {
    let path = triage.current()?.thread.anchor.gerrit_path();
    entries.iter().position(|entry| entry.path == path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gerrit::thread::assemble;
    use crate::gerrit::{Comment, Thread};
    use crate::triage::Decision;

    fn thread_on(path: &str, id: &str) -> Thread {
        let comment: Comment = serde_json::from_value(serde_json::json!({
            "id": id, "unresolved": true, "patch_set": 3, "line": 1,
        }))
        .unwrap();
        assemble(path, vec![comment]).remove(0)
    }

    fn triage_of(pairs: &[(&str, &str)]) -> Triage {
        let threads = pairs.iter().map(|(p, id)| thread_on(p, id)).collect();
        Triage::new(threads, &[], 0)
    }

    #[test]
    fn threads_are_grouped_by_document_with_counts() {
        let triage = triage_of(&[("a.rs", "c1"), ("a.rs", "c2"), ("b.rs", "c3")]);
        let entries = entries(&triage);

        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].path, "a.rs");
        assert_eq!(entries[0].total, 2);
        assert_eq!(entries[1].total, 1);
    }

    #[test]
    fn progress_tracks_decisions() {
        let mut triage = triage_of(&[("a.rs", "c1"), ("a.rs", "c2")]);
        assert_eq!(entries(&triage)[0].progress(), "0/2");
        assert!(!entries(&triage)[0].is_complete());

        triage.decide(Decision::Accept);
        assert_eq!(entries(&triage)[0].progress(), "1/2");

        triage.next();
        triage.decide(Decision::Reject);
        let entries = entries(&triage);
        assert_eq!(entries[0].progress(), "2/2");
        assert!(entries[0].is_complete());
    }

    /// A deferral is a decision for progress purposes — it has been looked at.
    #[test]
    fn a_deferred_thread_counts_as_decided_for_progress() {
        let mut triage = triage_of(&[("a.rs", "c1")]);
        triage.decide(Decision::Defer);
        assert_eq!(entries(&triage)[0].progress(), "1/1");
    }

    /// The entries a directory walk could never produce.
    #[test]
    fn pseudo_paths_appear_and_are_marked_as_non_files() {
        let triage = triage_of(&[
            ("a.rs", "c1"),
            ("/COMMIT_MSG", "m1"),
            ("/PATCHSET_LEVEL", "p1"),
        ]);
        let entries = entries(&triage);

        assert_eq!(entries.len(), 3);
        assert!(entries[0].is_file);
        assert!(!entries[1].is_file, "/COMMIT_MSG is not a file");
        assert!(!entries[2].is_file, "/PATCHSET_LEVEL is not a file");
        assert_eq!(entries[0].kind_marker(), " ");
        assert_eq!(entries[1].kind_marker(), "*");
    }

    #[test]
    fn order_follows_the_queue_not_the_alphabet() {
        let triage = triage_of(&[("z.rs", "c1"), ("a.rs", "c2")]);
        let entries = entries(&triage);
        assert_eq!(entries[0].path, "z.rs", "the queue's order, not sorted");
    }

    #[test]
    fn the_selected_entry_follows_the_cursor() {
        let mut triage = triage_of(&[("a.rs", "c1"), ("b.rs", "c2")]);
        let entries = entries(&triage);
        assert_eq!(selected(&triage, &entries), Some(0));
        triage.next();
        assert_eq!(selected(&triage, &entries), Some(1));
    }

    #[test]
    fn an_empty_queue_has_no_entries() {
        let triage = Triage::new(Vec::new(), &[], 0);
        assert!(entries(&triage).is_empty());
        assert_eq!(selected(&triage, &[]), None);
    }
}
