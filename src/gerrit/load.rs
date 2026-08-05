//! Loading a change into the threads the triage UI consumes.
//!
//! Ties the fetch together: change detail supplies the current patchset, the
//! two comment endpoints supply the raw comments, and thread assembly plus
//! patchset selection turn them into what triage sees. Drafts are absent by
//! construction — [`GerritApi`] has no method that could fetch them.

use super::api::{ChangeInfo, GerritApi};
use super::thread::{assemble, select_triagable};
use super::{Comment, GerritError, ThreadSet};

/// A change and its triagable threads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadedChange {
    pub change: ChangeInfo,
    pub threads: ThreadSet,
}

impl LoadedChange {
    /// Whether there is anything to triage. A change with no unresolved threads
    /// exits gracefully rather than opening an empty UI (design.md §14).
    pub fn is_empty(&self) -> bool {
        self.threads.threads.is_empty()
    }
}

/// Fetch a change and assemble its triagable threads.
///
/// Human and robot comments are merged per path before assembly, so a robot
/// comment and a human comment on the same file become separate threads unless
/// `in_reply_to` actually links them.
pub fn load_change(api: &impl GerritApi, change_id: &str) -> Result<LoadedChange, GerritError> {
    let change = api.change(change_id)?;
    let mut by_path = api.comments(change_id)?;
    for (path, robot) in api.robot_comments(change_id)? {
        by_path.entry(path).or_default().extend(robot);
    }

    let threads = assemble_all(by_path);
    let threads = select_triagable(threads, change.current_patch_set());
    Ok(LoadedChange { change, threads })
}

/// Assemble every path's comments into threads, in a stable path order.
fn assemble_all(by_path: std::collections::HashMap<String, Vec<Comment>>) -> Vec<super::Thread> {
    let mut paths: Vec<String> = by_path.keys().cloned().collect();
    paths.sort();
    let mut threads = Vec::new();
    for path in paths {
        let comments = by_path.get(&path).cloned().unwrap_or_default();
        threads.extend(assemble(&path, comments));
    }
    threads
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gerrit::{CommentAnchor, FakeGerrit};

    const CHANGE_BODY: &str = r#")]}'
{
  "id": "proj~main~I0123",
  "project": "proj",
  "branch": "main",
  "current_revision": "sha3",
  "revisions": {"sha3": {"_number": 3, "ref": "refs/changes/45/12345/3"}}
}"#;

    fn comment(id: &str, patch_set: u32, unresolved: bool) -> Comment {
        serde_json::from_value(serde_json::json!({
            "id": id, "patch_set": patch_set, "unresolved": unresolved,
            "updated": "2026-08-01 10:00", "line": 10,
        }))
        .unwrap()
    }

    fn robot(id: &str, patch_set: u32) -> Comment {
        serde_json::from_value(serde_json::json!({
            "id": id, "robot_id": "clippy", "patch_set": patch_set,
            "unresolved": true, "updated": "2026-08-01 10:00", "line": 20,
        }))
        .unwrap()
    }

    fn gerrit() -> FakeGerrit {
        FakeGerrit::from_change_json(CHANGE_BODY).unwrap()
    }

    #[test]
    fn a_change_loads_with_its_threads() {
        let api = gerrit().with_comments("src/a.rs", vec![comment("c1", 3, true)]);
        let loaded = load_change(&api, "12345").unwrap();
        assert_eq!(loaded.change.branch, "main");
        assert_eq!(loaded.threads.threads.len(), 1);
        assert!(!loaded.is_empty());
    }

    #[test]
    fn both_comment_endpoints_reach_triage() {
        let api = gerrit()
            .with_comments("src/a.rs", vec![comment("h1", 3, true)])
            .with_robot_comments("src/a.rs", vec![robot("r1", 3)]);
        let loaded = load_change(&api, "12345").unwrap();
        assert_eq!(loaded.threads.threads.len(), 2, "human + robot");
        assert!(loaded.threads.threads.iter().any(|t| t.is_robot()));
        assert!(loaded.threads.threads.iter().any(|t| !t.is_robot()));
    }

    #[test]
    fn a_robot_and_a_human_comment_on_one_file_stay_separate_threads() {
        let api = gerrit()
            .with_comments("src/a.rs", vec![comment("h1", 3, true)])
            .with_robot_comments("src/a.rs", vec![robot("r1", 3)]);
        let loaded = load_change(&api, "12345").unwrap();
        let roots: Vec<&str> = loaded
            .threads
            .threads
            .iter()
            .map(|t| t.root().id.as_str())
            .collect();
        assert_eq!(roots.len(), 2);
        assert!(roots.contains(&"h1") && roots.contains(&"r1"));
    }

    #[test]
    fn older_patchset_threads_are_skipped_and_counted_through_the_load() {
        let api = gerrit().with_comments(
            "src/a.rs",
            vec![comment("old", 1, true), comment("cur", 3, true)],
        );
        let loaded = load_change(&api, "12345").unwrap();
        assert_eq!(loaded.threads.threads.len(), 1);
        assert_eq!(loaded.threads.skipped_older_patchsets, 1);
    }

    #[test]
    fn a_change_with_nothing_unresolved_loads_empty() {
        let api = gerrit().with_comments("src/a.rs", vec![comment("done", 3, false)]);
        let loaded = load_change(&api, "12345").unwrap();
        assert!(loaded.is_empty(), "callers exit gracefully on this");
        assert_eq!(loaded.threads.skipped_older_patchsets, 0);
    }

    #[test]
    fn pseudo_paths_are_classified_through_the_load() {
        let api = gerrit()
            .with_comments("/COMMIT_MSG", vec![comment("m1", 3, true)])
            .with_comments("/PATCHSET_LEVEL", vec![comment("p1", 3, true)]);
        let loaded = load_change(&api, "12345").unwrap();
        let anchors: Vec<&CommentAnchor> =
            loaded.threads.threads.iter().map(|t| &t.anchor).collect();
        assert_eq!(anchors.len(), 2, "the two pseudo-paths are distinct keys");
        assert!(anchors.contains(&&CommentAnchor::CommitMessage));
        assert!(anchors.contains(&&CommentAnchor::ChangeLevel));
        assert!(
            anchors.iter().all(|a| a.file_path().is_none()),
            "a pseudo-path must never yield an on-disk path"
        );
        assert!(
            anchors.iter().all(|a| !a.is_editable_file()),
            "neither pseudo-path may reach an apply turn"
        );
    }

    #[test]
    fn thread_order_is_stable_across_loads() {
        let api = gerrit()
            .with_comments("src/z.rs", vec![comment("z1", 3, true)])
            .with_comments("src/a.rs", vec![comment("a1", 3, true)]);
        let first = load_change(&api, "12345").unwrap();
        let second = load_change(&api, "12345").unwrap();
        assert_eq!(first, second, "the triage queue must not reshuffle");
    }

    #[test]
    fn an_unknown_change_fails_before_anything_else() {
        let err = load_change(&FakeGerrit::default(), "98765").unwrap_err();
        assert!(matches!(err, GerritError::NoSuchChange { .. }));
        assert!(err.to_string().contains("98765"));
    }
}
