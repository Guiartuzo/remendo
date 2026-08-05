//! The document a thread is anchored to.
//!
//! The document pane is **polymorphic** (specs/comment-triage): a thread hangs
//! off a source file, off the commit message, or off the change as a whole. Only
//! the first exists on disk, so the other two are built rather than read — which
//! is what keeps a pseudo-path from ever reaching the filesystem.

use crate::apply::WorktreeFiles;
use crate::gerrit::anchor::commit_msg_line;
use crate::gerrit::{ChangeInfo, CommentAnchor, Thread};

/// A document to render read-only in the triage pane.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Document {
    /// What to show in the pane's header.
    pub title: String,
    /// The body, as lines.
    pub lines: Vec<String>,
    /// The 1-based line the thread is anchored at, when the document has one.
    pub anchor_line: Option<usize>,
    /// Whether syntax highlighting applies. Only real source files get it; the
    /// commit message and the change overview are prose.
    pub highlighted: bool,
}

impl Document {
    /// Build the document a thread is anchored to.
    ///
    /// `files` is only consulted for a real file anchor. The commit message and
    /// the change overview are synthesised from what Gerrit already told us, so
    /// neither pseudo-path is ever handed to the filesystem.
    pub fn for_thread(
        thread: &Thread,
        change: &ChangeInfo,
        commit_message: &str,
        files: &impl WorktreeFiles,
    ) -> Self {
        match &thread.anchor {
            CommentAnchor::File(path) => source_file(path, thread, files),
            CommentAnchor::CommitMessage => commit_message_document(thread, commit_message),
            CommentAnchor::ChangeLevel => change_overview(change),
        }
    }

    /// The 0-based index of the anchored line, for scrolling it into view.
    pub fn anchor_index(&self) -> Option<usize> {
        self.anchor_line.map(|line| line.saturating_sub(1))
    }
}

/// A source file read from the worktree. Unreadable files become a placeholder
/// rather than an error: the thread still needs triaging, and a deleted file is
/// a real state a reviewer must be able to see and decide about.
fn source_file(path: &str, thread: &Thread, files: &impl WorktreeFiles) -> Document {
    let lines = match files.read(path) {
        Ok(text) => text.lines().map(str::to_string).collect(),
        Err(_) => vec![format!("({path} could not be read from the worktree)")],
    };
    Document {
        title: path.to_string(),
        lines,
        anchor_line: thread.line().map(|line| line as usize),
        highlighted: true,
    }
}

/// The commit message, with the thread's line mapped off Gerrit's synthetic
/// header. A line addressing that header maps to nothing, so the anchor is
/// dropped rather than pointed somewhere arbitrary.
fn commit_message_document(thread: &Thread, commit_message: &str) -> Document {
    Document {
        title: "/COMMIT_MSG".to_string(),
        lines: commit_message.lines().map(str::to_string).collect(),
        anchor_line: thread
            .line()
            .and_then(|line| commit_msg_line(line as usize)),
        highlighted: false,
    }
}

/// A synthetic overview for a change-level thread, which anchors to nothing.
fn change_overview(change: &ChangeInfo) -> Document {
    Document {
        title: "/PATCHSET_LEVEL".to_string(),
        lines: vec![
            format!("Change {}", change.id),
            format!("Subject  {}", change.subject),
            format!("Project  {}", change.project),
            format!("Branch   {}", change.branch),
            format!(
                "Patchset {} ({})",
                change.current_patch_set(),
                change.current_revision
            ),
            String::new(),
            "This comment is on the change as a whole, not on any file.".to_string(),
        ],
        anchor_line: None,
        highlighted: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::apply::FakeWorktree;
    use crate::gerrit::Comment;
    use crate::gerrit::thread::assemble;

    fn thread_at(path: &str, line: Option<u32>) -> Thread {
        let comment: Comment = serde_json::from_value(serde_json::json!({
            "id": "c1", "unresolved": true, "patch_set": 3, "line": line,
        }))
        .unwrap();
        assemble(path, vec![comment]).remove(0)
    }

    fn change() -> ChangeInfo {
        serde_json::from_value(serde_json::json!({
            "id": "12345", "project": "proj", "branch": "main",
            "subject": "Tighten the parser", "current_revision": "sha3",
            "revisions": {"sha3": {"_number": 3, "ref": "refs/changes/45/12345/3"}},
        }))
        .unwrap()
    }

    const MESSAGE: &str = "Tighten the parser\n\nStrip the guard first.\n\nBug: 41827\n";

    #[test]
    fn a_file_thread_reads_the_worktree_and_is_highlighted() {
        let files = FakeWorktree::with_files(&[("a.rs", "fn a() {}\nfn b() {}\n")]);
        let doc = Document::for_thread(&thread_at("a.rs", Some(2)), &change(), MESSAGE, &files);

        assert_eq!(doc.title, "a.rs");
        assert_eq!(doc.lines, vec!["fn a() {}", "fn b() {}"]);
        assert_eq!(doc.anchor_line, Some(2));
        assert_eq!(doc.anchor_index(), Some(1));
        assert!(doc.highlighted);
    }

    /// A deleted or unreadable file is a real state to triage, not a crash.
    #[test]
    fn an_unreadable_file_becomes_a_placeholder() {
        let files = FakeWorktree::default();
        let doc = Document::for_thread(&thread_at("gone.rs", Some(1)), &change(), MESSAGE, &files);
        assert_eq!(doc.lines.len(), 1);
        assert!(doc.lines[0].contains("gone.rs"));
    }

    /// Gerrit numbers commit-message lines from its own rendering, which
    /// prepends six header lines. Line 7 is the subject.
    #[test]
    fn a_commit_message_thread_maps_past_gerrits_header() {
        let files = FakeWorktree::default();
        let doc = Document::for_thread(
            &thread_at("/COMMIT_MSG", Some(7)),
            &change(),
            MESSAGE,
            &files,
        );

        assert_eq!(doc.title, "/COMMIT_MSG");
        assert_eq!(doc.lines[0], "Tighten the parser");
        assert_eq!(doc.anchor_line, Some(1), "the subject");
        assert!(!doc.highlighted, "a commit message is prose");
    }

    #[test]
    fn a_comment_on_gerrits_synthetic_header_anchors_nowhere() {
        let files = FakeWorktree::default();
        let doc = Document::for_thread(
            &thread_at("/COMMIT_MSG", Some(3)),
            &change(),
            MESSAGE,
            &files,
        );
        assert_eq!(
            doc.anchor_line, None,
            "rather than pointing somewhere wrong"
        );
    }

    #[test]
    fn a_change_level_thread_gets_a_synthetic_overview() {
        let files = FakeWorktree::default();
        let doc = Document::for_thread(
            &thread_at("/PATCHSET_LEVEL", None),
            &change(),
            MESSAGE,
            &files,
        );

        assert_eq!(doc.title, "/PATCHSET_LEVEL");
        assert!(doc.lines.iter().any(|l| l.contains("Tighten the parser")));
        assert!(doc.lines.iter().any(|l| l.contains("main")));
        assert_eq!(doc.anchor_line, None, "it anchors to nothing");
    }

    /// The property the whole polymorphism exists for.
    #[test]
    fn no_pseudo_path_is_ever_read_from_the_worktree() {
        // A worktree that panics if read at all.
        struct NeverRead;
        impl WorktreeFiles for NeverRead {
            fn read(&self, path: &str) -> Result<String, crate::apply::ApplyError> {
                panic!("the filesystem was consulted for {path}");
            }
            fn write(&self, _: &str, _: &str) -> Result<(), crate::apply::ApplyError> {
                unreachable!()
            }
        }

        for path in ["/COMMIT_MSG", "/PATCHSET_LEVEL"] {
            Document::for_thread(&thread_at(path, Some(7)), &change(), MESSAGE, &NeverRead);
        }
    }
}
