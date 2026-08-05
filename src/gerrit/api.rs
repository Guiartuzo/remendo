//! The `GerritApi` trait: the Gerrit operations Remendo needs.
//!
//! Two absences are deliberate and load-bearing (design.md §14):
//!
//! * **No `drafts` method.** Excluding draft comments was a decision, and a
//!   trait carrying no method for them cannot be talked into fetching them by a
//!   later call site.
//! * **No `resolve` method.** Gerrit has no mark-comment-resolved mutation — a
//!   thread is resolved by replying with `unresolved: false` — so modelling one
//!   would invite code that calls a mutation that does not exist.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use super::{Comment, GerritError};

/// The change fields Remendo needs.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ChangeInfo {
    /// Gerrit's change id (the long `project~branch~Ixxxx` form or the number).
    #[serde(default)]
    pub id: String,

    /// The project this change belongs to. Validated against the clone the user
    /// launched in — a change id names no repository (design.md §13).
    #[serde(default)]
    pub project: String,

    /// The target branch. Finalize pushes to `refs/for/<branch>`; the worktree
    /// is on a detached HEAD, so there is no local branch name to read.
    #[serde(default)]
    pub branch: String,

    /// The current revision's commit sha. Both the pre-push check and the
    /// verdict cache key depend on it.
    #[serde(default)]
    pub current_revision: String,

    #[serde(default)]
    pub subject: String,

    /// Revision detail, keyed by sha. Used to find the current patchset number.
    #[serde(default)]
    pub revisions: HashMap<String, RevisionInfo>,
}

impl ChangeInfo {
    /// The patchset number of `current_revision`, or 0 when Gerrit did not
    /// return revision detail. Threads anchored below this are skipped.
    pub fn current_patch_set(&self) -> u32 {
        self.revisions
            .get(&self.current_revision)
            .map(|r| r.number)
            .unwrap_or(0)
    }

    /// The ref carrying the current patchset, for `git fetch`.
    pub fn current_revision_ref(&self) -> Option<&str> {
        self.revisions
            .get(&self.current_revision)?
            .git_ref
            .as_deref()
    }
}

/// One revision (patchset) of a change.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct RevisionInfo {
    #[serde(rename = "_number", default)]
    pub number: u32,
    #[serde(rename = "ref", default)]
    pub git_ref: Option<String>,
}

/// One comment's fate in the batched review post.
///
/// Resolution and reply are the same operation differing only in `unresolved`,
/// because Gerrit provides no separate resolve mutation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ReviewComment {
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
    /// The thread's **last** comment id — replies attach to the end of the
    /// exchange, not its opening comment (design.md §13).
    pub in_reply_to: String,
    pub message: String,
    pub unresolved: bool,
}

/// The single batched review post issued after a successful push.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct ReviewInput {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    /// Comment fates, keyed by path. Accepted and hand-fixed threads carry
    /// `unresolved: false`; rejected ones carry the approved reply and
    /// `unresolved: true`. Skipped threads are absent entirely.
    pub comments: HashMap<String, Vec<ReviewComment>>,
}

/// The Gerrit REST operations Remendo needs.
pub trait GerritApi {
    /// Fetch a change with its current revision, branch, project and revisions.
    fn change(&self, change_id: &str) -> Result<ChangeInfo, GerritError>;

    /// Published inline comments, keyed by Gerrit comment path.
    fn comments(&self, change_id: &str) -> Result<HashMap<String, Vec<Comment>>, GerritError>;

    /// Robot comments, from Gerrit's separate robot-comment endpoint. A second
    /// fetch path, not a filter flag (design.md §13).
    fn robot_comments(&self, change_id: &str)
    -> Result<HashMap<String, Vec<Comment>>, GerritError>;

    /// The one batched review post carrying every comment fate. Issued only
    /// after a new patchset has been pushed successfully.
    fn post_review(&self, change_id: &str, review: &ReviewInput) -> Result<(), GerritError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn change_from(json: &str) -> ChangeInfo {
        serde_json::from_str(json).expect("fixture parses")
    }

    #[test]
    fn a_change_exposes_the_fields_finalize_needs() {
        let change = change_from(
            r#"{
                "id": "proj~main~I123",
                "project": "proj",
                "branch": "main",
                "current_revision": "sha3",
                "subject": "Fix the thing",
                "revisions": {
                    "sha3": {"_number": 3, "ref": "refs/changes/45/12345/3"}
                }
            }"#,
        );
        assert_eq!(change.project, "proj");
        assert_eq!(change.branch, "main");
        assert_eq!(change.current_patch_set(), 3);
        assert_eq!(
            change.current_revision_ref(),
            Some("refs/changes/45/12345/3")
        );
    }

    #[test]
    fn a_change_without_revision_detail_degrades_rather_than_panicking() {
        let change = change_from(r#"{"project":"p","branch":"main","current_revision":"sha"}"#);
        assert_eq!(change.current_patch_set(), 0);
        assert_eq!(change.current_revision_ref(), None);
    }

    #[test]
    fn an_older_revision_does_not_supply_the_current_patch_set() {
        let change = change_from(
            r#"{
                "current_revision": "sha3",
                "revisions": {
                    "sha1": {"_number": 1, "ref": "refs/changes/45/12345/1"},
                    "sha3": {"_number": 3, "ref": "refs/changes/45/12345/3"}
                }
            }"#,
        );
        assert_eq!(change.current_patch_set(), 3, "reads the CURRENT revision");
    }

    #[test]
    fn a_review_serializes_the_shape_gerrit_expects() {
        let mut review = ReviewInput::default();
        review.comments.insert(
            "src/a.rs".into(),
            vec![ReviewComment {
                path: "src/a.rs".into(),
                line: Some(47),
                in_reply_to: "c9".into(),
                message: "Fixed in this patchset.".into(),
                unresolved: false,
            }],
        );
        let json = serde_json::to_value(&review).unwrap();
        let comment = &json["comments"]["src/a.rs"][0];
        assert_eq!(comment["in_reply_to"], "c9");
        assert_eq!(comment["unresolved"], false);
        assert!(json.get("message").is_none(), "absent, not null");
    }

    #[test]
    fn a_change_level_review_comment_omits_the_line() {
        let comment = ReviewComment {
            path: "/PATCHSET_LEVEL".into(),
            line: None,
            in_reply_to: "c1".into(),
            message: "Disagree, see below.".into(),
            unresolved: true,
        };
        let json = serde_json::to_value(&comment).unwrap();
        assert!(json.get("line").is_none(), "no null line for change-level");
        assert_eq!(json["unresolved"], true);
    }
}
