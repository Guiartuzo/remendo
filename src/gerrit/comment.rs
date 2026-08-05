//! A single Gerrit comment, as fetched and as modelled.
//!
//! The wire shape and the domain shape are deliberately the same type here: the
//! REST payload is already close to what the rest of the application wants, and
//! a second near-identical struct would be duplication rather than a seam. What
//! *is* converted is the `path`, which becomes a [`CommentAnchor`] at the point
//! a thread is built, so no pseudo-path ever reaches the filesystem.

use serde::Deserialize;

/// A character range within a file, when a comment spans more than a line.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct CommentRange {
    pub start_line: u32,
    pub start_character: u32,
    pub end_line: u32,
    pub end_character: u32,
}

/// One comment on a change.
///
/// Covers both human comments and robot comments: Gerrit serves them from
/// separate endpoints with the robot payload adding fields, so a robot comment
/// deserializes into this same struct with `robot_id` populated.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Comment {
    pub id: String,

    /// Set when this comment replies to another. `None` marks a thread root.
    #[serde(default)]
    pub in_reply_to: Option<String>,

    /// Gerrit's `updated` timestamp, used to order a thread and to break ties
    /// when a thread branches.
    #[serde(default)]
    pub updated: String,

    #[serde(default)]
    pub author: Author,

    #[serde(default)]
    pub message: String,

    /// Per-comment resolved flag. **Not** the thread's state — that is the last
    /// comment's flag (see [`crate::gerrit::Thread::is_unresolved`]). Reading
    /// this per comment is the bug design.md §13 exists to prevent.
    #[serde(default)]
    pub unresolved: bool,

    /// The patchset this comment was written against. A comment from an earlier
    /// patchset has a line anchor addressing code that may no longer exist.
    #[serde(default)]
    pub patch_set: u32,

    #[serde(default)]
    pub line: Option<u32>,

    #[serde(default)]
    pub range: Option<CommentRange>,

    /// Present only on robot comments; identifies the linter or CI job.
    #[serde(default)]
    pub robot_id: Option<String>,

    /// A robot's machine-applicable fix, when it supplied one. Unused in v0 —
    /// whether to apply it instead of spending a Claude turn is an open design
    /// question (design.md §12 item 9) — but it is parsed rather than dropped so
    /// the decision stays available.
    #[serde(default)]
    pub fix_suggestions: Vec<serde_json::Value>,
}

impl Comment {
    /// Whether this comment came from a linter or CI job rather than a person.
    pub fn is_robot(&self) -> bool {
        self.robot_id.is_some()
    }

    /// The author's display name, or a placeholder when Gerrit omitted it.
    pub fn author_name(&self) -> &str {
        self.author.name.as_deref().unwrap_or("(unknown)")
    }
}

/// A comment's author. Only the fields the UI shows are modelled; the identity
/// is used for provenance filtering and for display, never for the verdict —
/// verdicts are judged on technical merit (specs/comment-triage).
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct Author {
    #[serde(rename = "_account_id", default)]
    pub account_id: Option<u64>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub email: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(json: &str) -> Comment {
        serde_json::from_str(json).expect("fixture parses")
    }

    #[test]
    fn a_minimal_comment_parses_with_defaults() {
        let c = parse(r#"{"id":"abc"}"#);
        assert_eq!(c.id, "abc");
        assert_eq!(c.in_reply_to, None);
        assert!(!c.unresolved);
        assert!(!c.is_robot());
        assert_eq!(c.author_name(), "(unknown)");
    }

    #[test]
    fn a_full_human_comment_parses() {
        let c = parse(
            r#"{
                "id": "c1",
                "in_reply_to": "c0",
                "updated": "2026-08-01 10:00:00.000000000",
                "author": {"_account_id": 7, "name": "Rafa"},
                "message": "this is O(n^2)",
                "unresolved": true,
                "patch_set": 3,
                "line": 47
            }"#,
        );
        assert_eq!(c.in_reply_to.as_deref(), Some("c0"));
        assert_eq!(c.author_name(), "Rafa");
        assert_eq!(c.patch_set, 3);
        assert_eq!(c.line, Some(47));
        assert!(c.unresolved);
        assert!(!c.is_robot());
    }

    #[test]
    fn a_robot_comment_is_recognized_and_keeps_its_fix() {
        let c = parse(
            r#"{
                "id": "r1",
                "robot_id": "clippy",
                "message": "needless clone",
                "fix_suggestions": [{"description": "remove the clone"}]
            }"#,
        );
        assert!(c.is_robot());
        assert_eq!(c.robot_id.as_deref(), Some("clippy"));
        assert_eq!(c.fix_suggestions.len(), 1, "fix is parsed, not dropped");
    }

    #[test]
    fn a_ranged_comment_parses_its_range() {
        let c = parse(
            r#"{
                "id": "c2",
                "range": {
                    "start_line": 10, "start_character": 4,
                    "end_line": 12, "end_character": 0
                }
            }"#,
        );
        let range = c.range.expect("range present");
        assert_eq!(range.start_line, 10);
        assert_eq!(range.end_line, 12);
    }

    #[test]
    fn unknown_fields_do_not_break_parsing() {
        // Gerrit versions differ in what they send; an unmodelled field must
        // not fail the fetch.
        let c = parse(r#"{"id":"c3","some_future_field":{"nested":true}}"#);
        assert_eq!(c.id, "c3");
    }
}
