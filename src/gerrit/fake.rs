//! A named fake [`GerritApi`] serving captured fixtures.
//!
//! `config.yaml` requires external I/O to be mocked with a named fake type
//! implementing the project's own trait. This one is loaded from JSON that
//! matches real Gerrit responses, and records the reviews posted to it so a
//! test can assert the batched post's exact contents.

use std::cell::RefCell;
use std::collections::HashMap;

use super::api::{ChangeInfo, GerritApi, ReviewInput};
use super::{Comment, GerritError};

/// A [`GerritApi`] answering from in-memory fixtures.
#[derive(Debug, Default)]
pub struct FakeGerrit {
    change: Option<ChangeInfo>,
    comments: HashMap<String, Vec<Comment>>,
    robot_comments: HashMap<String, Vec<Comment>>,
    /// Reviews posted, in order. Finalize must issue exactly one.
    posted: RefCell<Vec<ReviewInput>>,
}

impl FakeGerrit {
    /// A fake serving `change` and nothing else.
    pub fn serving(change: ChangeInfo) -> Self {
        Self {
            change: Some(change),
            ..Self::default()
        }
    }

    /// A fake built from a captured `/changes/{id}/detail` body, XSSI guard and
    /// all — so fixtures can be pasted straight from a real response.
    pub fn from_change_json(body: &str) -> Result<Self, GerritError> {
        Ok(Self::serving(super::response::decode(body)?))
    }

    /// Add published comments for one path.
    pub fn with_comments(mut self, path: &str, comments: Vec<Comment>) -> Self {
        self.comments.insert(path.to_string(), comments);
        self
    }

    /// Add robot comments for one path.
    pub fn with_robot_comments(mut self, path: &str, comments: Vec<Comment>) -> Self {
        self.robot_comments.insert(path.to_string(), comments);
        self
    }

    /// The reviews posted to this fake, in order.
    pub fn posted_reviews(&self) -> Vec<ReviewInput> {
        self.posted.borrow().clone()
    }
}

impl GerritApi for FakeGerrit {
    fn change(&self, change_id: &str) -> Result<ChangeInfo, GerritError> {
        self.change
            .clone()
            .ok_or_else(|| GerritError::NoSuchChange {
                change_id: change_id.to_string(),
            })
    }

    fn comments(&self, _change_id: &str) -> Result<HashMap<String, Vec<Comment>>, GerritError> {
        Ok(self.comments.clone())
    }

    fn robot_comments(
        &self,
        _change_id: &str,
    ) -> Result<HashMap<String, Vec<Comment>>, GerritError> {
        Ok(self.robot_comments.clone())
    }

    fn post_review(&self, _change_id: &str, review: &ReviewInput) -> Result<(), GerritError> {
        self.posted.borrow_mut().push(review.clone());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A captured change body, XSSI guard included.
    const CHANGE_BODY: &str = r#")]}'
{
  "id": "proj~main~I0123456789abcdef",
  "project": "proj",
  "branch": "main",
  "subject": "Tighten the response parser",
  "current_revision": "d3adb33f",
  "revisions": {
    "c0ffee11": {"_number": 1, "ref": "refs/changes/45/12345/1"},
    "d3adb33f": {"_number": 3, "ref": "refs/changes/45/12345/3"}
  }
}"#;

    #[test]
    fn a_captured_change_body_loads_through_the_xssi_guard() {
        let gerrit = FakeGerrit::from_change_json(CHANGE_BODY).unwrap();
        let change = gerrit.change("12345").unwrap();
        assert_eq!(change.project, "proj");
        assert_eq!(change.branch, "main");
        assert_eq!(change.current_patch_set(), 3);
        assert_eq!(
            change.current_revision_ref(),
            Some("refs/changes/45/12345/3")
        );
    }

    #[test]
    fn an_empty_fake_reports_the_change_id_it_was_asked_for() {
        let err = FakeGerrit::default().change("98765").unwrap_err();
        assert!(err.to_string().contains("98765"), "{err}");
    }

    #[test]
    fn comments_and_robot_comments_are_separate_paths() {
        let human: Comment = serde_json::from_value(serde_json::json!({"id": "h1"})).unwrap();
        let robot: Comment =
            serde_json::from_value(serde_json::json!({"id": "r1", "robot_id": "clippy"})).unwrap();
        let gerrit = FakeGerrit::from_change_json(CHANGE_BODY)
            .unwrap()
            .with_comments("src/a.rs", vec![human])
            .with_robot_comments("src/a.rs", vec![robot]);

        assert_eq!(gerrit.comments("1").unwrap()["src/a.rs"].len(), 1);
        assert_eq!(gerrit.robot_comments("1").unwrap()["src/a.rs"].len(), 1);
        assert!(
            !gerrit.comments("1").unwrap()["src/a.rs"][0].is_robot(),
            "the endpoints must not bleed into each other"
        );
    }

    #[test]
    fn posted_reviews_are_recorded_for_assertion() {
        let gerrit = FakeGerrit::from_change_json(CHANGE_BODY).unwrap();
        assert!(gerrit.posted_reviews().is_empty());
        gerrit
            .post_review("12345", &ReviewInput::default())
            .unwrap();
        assert_eq!(
            gerrit.posted_reviews().len(),
            1,
            "finalize must issue exactly one batched post"
        );
    }
}
