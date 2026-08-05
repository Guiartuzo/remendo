//! A named fake [`ClaudeDriver`] for tests.
//!
//! It holds the same [`FakeWorktree`] the apply loop reads, so an apply turn
//! mutates files *behind the loop's back* — exactly the relationship the real
//! driver has, where Claude's `Edit` writes to disk and the loop discovers the
//! change by re-reading. A driver that returned contents instead would test a
//! contract Remendo does not have.

use std::cell::RefCell;
use std::path::Path;

use super::{ClaudeDriver, DriverError, Envelope, SessionId};
use crate::apply::FakeWorktree;

/// A scripted [`ClaudeDriver`] over an in-memory worktree.
#[derive(Debug, Clone)]
pub struct FakeDriver {
    worktree: FakeWorktree,
    /// Queued edits, applied one per apply turn in order.
    edits: RefCell<Vec<(String, String)>>,
    /// Prompts received, for asserting what a turn was told.
    turns: RefCell<Vec<String>>,
    /// Canned reply text for [`reply_turn`](ClaudeDriver::reply_turn).
    reply: String,
    /// When set, every turn returns an envelope reporting failure.
    fails: bool,
    /// Cost reported per turn.
    cost_per_turn: f64,
}

impl FakeDriver {
    /// A driver editing `worktree`.
    pub fn new(worktree: FakeWorktree) -> Self {
        Self {
            worktree,
            edits: RefCell::new(Vec::new()),
            turns: RefCell::new(Vec::new()),
            reply: "Thanks — I disagree, see below.".to_string(),
            fails: false,
            cost_per_turn: 0.0,
        }
    }

    /// Queue what the next apply turn writes to `path`.
    pub fn editing(self, path: &str, contents: &str) -> Self {
        self.edits
            .borrow_mut()
            .push((path.to_string(), contents.to_string()));
        self
    }

    /// Make every turn report failure through its envelope.
    pub fn failing(mut self) -> Self {
        self.fails = true;
        self
    }

    /// Report `usd` for each turn, so cost accumulation can be asserted.
    pub fn costing(mut self, usd: f64) -> Self {
        self.cost_per_turn = usd;
        self
    }

    /// Set the text `reply_turn` returns.
    pub fn replying(mut self, reply: &str) -> Self {
        self.reply = reply.to_string();
        self
    }

    /// The prompts this driver was given, in order.
    pub fn turns(&self) -> Vec<String> {
        self.turns.borrow().clone()
    }

    /// Wrap a payload in an envelope honouring [`failing`](Self::failing).
    fn envelope<T>(&self, payload: T) -> Envelope<T> {
        Envelope {
            is_error: self.fails,
            total_cost_usd: self.cost_per_turn,
            structured_output: (!self.fails).then_some(payload),
        }
    }
}

impl ClaudeDriver for FakeDriver {
    fn verdict_turn(
        &self,
        _session: &SessionId,
        prompt: &str,
    ) -> Result<Envelope<serde_json::Value>, DriverError> {
        self.turns.borrow_mut().push(prompt.to_string());
        Ok(self.envelope(serde_json::json!([])))
    }

    fn apply_turn(
        &self,
        _session: &SessionId,
        prompt: &str,
        _worktree: &Path,
    ) -> Result<Envelope<()>, DriverError> {
        self.turns.borrow_mut().push(prompt.to_string());
        if self.fails {
            return Ok(self.envelope(()));
        }
        // Edit the worktree, as Claude's `Edit` tool would. A turn with no
        // queued edit leaves the file alone, which models a turn that decided
        // nothing needed changing.
        if !self.edits.borrow().is_empty() {
            let (path, contents) = self.edits.borrow_mut().remove(0);
            self.worktree.set(&path, &contents);
        }
        Ok(self.envelope(()))
    }

    fn reply_turn(
        &self,
        _session: &SessionId,
        prompt: &str,
    ) -> Result<Envelope<String>, DriverError> {
        self.turns.borrow_mut().push(prompt.to_string());
        Ok(self.envelope(self.reply.clone()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::apply::WorktreeFiles;

    fn session() -> SessionId {
        SessionId("s".into())
    }

    #[test]
    fn an_apply_turn_edits_the_shared_worktree() {
        let wt = FakeWorktree::with_files(&[("a.rs", "before\n")]);
        let driver = FakeDriver::new(wt.clone()).editing("a.rs", "after\n");

        driver
            .apply_turn(&session(), "do it", Path::new("/wt"))
            .unwrap();

        assert_eq!(wt.read("a.rs").unwrap(), "after\n");
    }

    #[test]
    fn queued_edits_are_applied_in_order() {
        let wt = FakeWorktree::with_files(&[("a.rs", "0\n")]);
        let driver = FakeDriver::new(wt.clone())
            .editing("a.rs", "1\n")
            .editing("a.rs", "2\n");

        driver.apply_turn(&session(), "", Path::new("/wt")).unwrap();
        assert_eq!(wt.read("a.rs").unwrap(), "1\n");
        driver.apply_turn(&session(), "", Path::new("/wt")).unwrap();
        assert_eq!(wt.read("a.rs").unwrap(), "2\n");
    }

    /// A turn that decides nothing needs changing is a real outcome, not a bug.
    #[test]
    fn a_turn_with_no_queued_edit_leaves_the_file_alone() {
        let wt = FakeWorktree::with_files(&[("a.rs", "unchanged\n")]);
        let driver = FakeDriver::new(wt.clone());

        driver.apply_turn(&session(), "", Path::new("/wt")).unwrap();

        assert_eq!(wt.read("a.rs").unwrap(), "unchanged\n");
    }

    #[test]
    fn a_failing_driver_reports_through_the_envelope() {
        let wt = FakeWorktree::with_files(&[("a.rs", "before\n")]);
        let driver = FakeDriver::new(wt.clone())
            .editing("a.rs", "after\n")
            .failing();

        let envelope = driver.apply_turn(&session(), "", Path::new("/wt")).unwrap();

        assert!(envelope.is_error);
        assert!(envelope.payload("apply").is_err());
        assert_eq!(
            wt.read("a.rs").unwrap(),
            "before\n",
            "a failed turn must not have edited"
        );
    }

    #[test]
    fn prompts_are_recorded_in_order() {
        let driver = FakeDriver::new(FakeWorktree::default());
        driver.verdict_turn(&session(), "adjudicate").unwrap();
        driver.reply_turn(&session(), "draft a reply").unwrap();
        assert_eq!(driver.turns(), vec!["adjudicate", "draft a reply"]);
    }

    #[test]
    fn cost_is_reported_per_turn() {
        let driver = FakeDriver::new(FakeWorktree::default()).costing(0.14);
        let envelope = driver.verdict_turn(&session(), "").unwrap();
        assert!((envelope.total_cost_usd - 0.14).abs() < 1e-9);
    }

    #[test]
    fn a_reply_turn_returns_its_canned_text() {
        let driver = FakeDriver::new(FakeWorktree::default()).replying("Bounded at 8 items.");
        let reply = driver
            .reply_turn(&session(), "")
            .unwrap()
            .payload("reply")
            .unwrap();
        assert_eq!(reply, "Bounded at 8 items.");
    }
}
