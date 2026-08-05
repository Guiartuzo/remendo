//! What a comment is anchored to.
//!
//! Gerrit addresses comments by a `path` field, but two of its values are not
//! files: `/COMMIT_MSG` is the commit message and `/PATCHSET_LEVEL` is the
//! change as a whole. Treating either as a path yields a worktree lookup for a
//! file that cannot exist, or worse a directory derived from a leading slash.
//! Classifying once, here, is what keeps that from being possible downstream.

/// Gerrit's pseudo-path for a comment on the commit message.
const COMMIT_MSG_PATH: &str = "/COMMIT_MSG";

/// Gerrit's pseudo-path for a change-level comment.
const PATCHSET_LEVEL_PATH: &str = "/PATCHSET_LEVEL";

/// Lines Gerrit prepends when it renders a commit message for commenting:
/// `Parent`, `Author`, `AuthorDate`, `Commit`, `CommitDate`, then a blank line.
/// A comment's line number counts from the top of *that* rendering, so mapping
/// it onto the real message means subtracting these.
const COMMIT_MSG_HEADER_LINES: usize = 6;

/// What a comment hangs off.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommentAnchor {
    /// A real file in the worktree, at a repository-relative path.
    File(String),
    /// The commit message. Carries no path — the message is not a file.
    CommitMessage,
    /// The change as a whole. Carries no path and no line.
    ChangeLevel,
}

impl CommentAnchor {
    /// Classify a Gerrit comment `path`.
    ///
    /// ```
    /// # use remendo::gerrit::anchor::CommentAnchor;
    /// assert_eq!(
    ///     CommentAnchor::classify("src/main.rs"),
    ///     CommentAnchor::File("src/main.rs".into())
    /// );
    /// assert_eq!(CommentAnchor::classify("/COMMIT_MSG"), CommentAnchor::CommitMessage);
    /// ```
    pub fn classify(path: &str) -> Self {
        match path {
            COMMIT_MSG_PATH => CommentAnchor::CommitMessage,
            PATCHSET_LEVEL_PATH => CommentAnchor::ChangeLevel,
            other => CommentAnchor::File(other.to_string()),
        }
    }

    /// The worktree-relative file path, or `None` for the two pseudo-paths.
    ///
    /// This is the only way to get a path out of an anchor, so a caller cannot
    /// accidentally hand `/COMMIT_MSG` to the filesystem.
    ///
    /// ```
    /// # use remendo::gerrit::anchor::CommentAnchor;
    /// assert_eq!(CommentAnchor::classify("a.rs").file_path(), Some("a.rs"));
    /// assert_eq!(CommentAnchor::classify("/PATCHSET_LEVEL").file_path(), None);
    /// ```
    pub fn file_path(&self) -> Option<&str> {
        match self {
            CommentAnchor::File(path) => Some(path),
            CommentAnchor::CommitMessage | CommentAnchor::ChangeLevel => None,
        }
    }

    /// Whether an apply turn can act on this anchor. Only real files can be
    /// edited: commit-message comments route into the finalize amend's message,
    /// and change-level comments are reply-only (design.md §7/§8).
    pub fn is_editable_file(&self) -> bool {
        matches!(self, CommentAnchor::File(_))
    }

    /// The path string Gerrit addresses this anchor by, for posting back.
    ///
    /// The inverse of [`classify`](Self::classify). Distinct from
    /// [`file_path`](Self::file_path) on purpose: this one always yields a
    /// string, so it must never be used to reach the filesystem.
    ///
    /// ```
    /// # use remendo::gerrit::anchor::CommentAnchor;
    /// assert_eq!(CommentAnchor::CommitMessage.gerrit_path(), "/COMMIT_MSG");
    /// assert_eq!(CommentAnchor::classify("a.rs").gerrit_path(), "a.rs");
    /// ```
    pub fn gerrit_path(&self) -> &str {
        match self {
            CommentAnchor::File(path) => path,
            CommentAnchor::CommitMessage => COMMIT_MSG_PATH,
            CommentAnchor::ChangeLevel => PATCHSET_LEVEL_PATH,
        }
    }
}

/// Map a comment's line number on Gerrit's rendered commit message onto a line
/// of the real message, accounting for the synthetic header Gerrit prepends.
///
/// Returns `None` for lines that address the header itself, which correspond to
/// no line of the real message.
///
/// ```
/// # use remendo::gerrit::anchor::commit_msg_line;
/// // Line 7 of the rendering is the first line of the real message.
/// assert_eq!(commit_msg_line(7), Some(1));
/// // Lines 1..=6 are Gerrit's Parent/Author/Commit header.
/// assert_eq!(commit_msg_line(3), None);
/// ```
pub fn commit_msg_line(rendered_line: usize) -> Option<usize> {
    rendered_line
        .checked_sub(COMMIT_MSG_HEADER_LINES)
        .filter(|line| *line > 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_normal_path_is_a_file() {
        assert_eq!(
            CommentAnchor::classify("src/gerrit/mod.rs"),
            CommentAnchor::File("src/gerrit/mod.rs".into())
        );
    }

    #[test]
    fn the_two_pseudo_paths_are_classified_as_such() {
        assert_eq!(
            CommentAnchor::classify("/COMMIT_MSG"),
            CommentAnchor::CommitMessage
        );
        assert_eq!(
            CommentAnchor::classify("/PATCHSET_LEVEL"),
            CommentAnchor::ChangeLevel
        );
    }

    /// The whole point of the type: neither pseudo-path can yield something the
    /// filesystem would accept, and a leading slash cannot become a directory.
    #[test]
    fn no_on_disk_path_comes_out_of_a_pseudo_path() {
        for path in ["/COMMIT_MSG", "/PATCHSET_LEVEL"] {
            let anchor = CommentAnchor::classify(path);
            assert_eq!(anchor.file_path(), None, "{path} yielded a path");
            assert!(!anchor.is_editable_file(), "{path} looked editable");
        }
    }

    #[test]
    fn a_file_whose_name_merely_resembles_a_pseudo_path_is_still_a_file() {
        // Real paths are repository-relative and never carry a leading slash,
        // so these are ordinary files rather than pseudo-paths.
        assert!(CommentAnchor::classify("COMMIT_MSG").is_editable_file());
        assert!(CommentAnchor::classify("docs/COMMIT_MSG.md").is_editable_file());
    }

    #[test]
    fn classification_round_trips_through_the_gerrit_path() {
        for path in ["src/a.rs", "/COMMIT_MSG", "/PATCHSET_LEVEL"] {
            assert_eq!(CommentAnchor::classify(path).gerrit_path(), path);
        }
    }

    /// `gerrit_path` always yields a string and `file_path` does not; the split
    /// is what keeps a pseudo-path postable but never openable.
    #[test]
    fn a_postable_path_is_not_an_openable_path() {
        let anchor = CommentAnchor::ChangeLevel;
        assert_eq!(anchor.gerrit_path(), "/PATCHSET_LEVEL");
        assert_eq!(anchor.file_path(), None);
    }

    #[test]
    fn commit_message_lines_shift_past_gerrits_header() {
        assert_eq!(commit_msg_line(7), Some(1)); // subject
        assert_eq!(commit_msg_line(8), Some(2));
        assert_eq!(commit_msg_line(20), Some(14));
    }

    #[test]
    fn lines_inside_the_header_map_to_nothing() {
        for rendered in 1..=6 {
            assert_eq!(commit_msg_line(rendered), None, "line {rendered}");
        }
        assert_eq!(commit_msg_line(0), None);
    }
}
