//! The triage queue: what the human decides, and how it becomes finalize's input.
//!
//! v0 is **manual only** (rung 0). Nothing is auto-accepted or auto-rejected;
//! the verdict informs the human and the human governs (design.md §8).
//!
//! Two rules shape the state machine:
//!
//! * **Deferral makes the queue non-linear.** "Move to the next comment" and
//!   "move to the next *undecided* comment" are different operations, because
//!   after a few deferrals the adjacent comment is usually already decided.
//! * **Triage ends at an explicit gate, never by running out.** Reaching the
//!   last comment does not finish anything. At the gate, deferral stops meaning
//!   "not yet" and starts meaning "not doing it", and stating the count is what
//!   keeps that transition from happening silently.

pub mod dependencies;
pub mod document;

pub use dependencies::{SharedDependency, collate};
pub use document::Document;

use crate::gerrit::Thread;
use crate::submit::{Fate, TriagedThread};
use crate::verdict::Verdict;

/// What the human decided about a thread.
///
/// Distinct from [`Fate`]: a `Decision` is what the human pressed, a `Fate` is
/// what Gerrit is told. `Defer` has no `Fate` because it never survives the
/// gate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    Accept,
    Reject,
    /// Revisit later. At the completion gate this becomes skipped.
    Defer,
    /// Already fixed in the user's own editor: resolves without an apply turn
    /// and without a confirm-diff.
    FixedByHand,
}

/// One thread in the triage queue, with everything decided about it so far.
#[derive(Debug, Clone, PartialEq)]
pub struct TriageItem {
    pub thread: Thread,
    /// Claude's adjudication, or `None` when the verdict pass could not produce
    /// one. **Unadjudicated is not the same as empty**: a missing verdict must
    /// stay visibly missing rather than read as "nothing to say" (design.md §14).
    pub verdict: Option<Verdict>,
    pub decision: Option<Decision>,
    /// The comment's prose after the user edited it, which is what feeds the
    /// apply turn.
    pub edited_prose: Option<String>,
    /// The reply approved during triage for a rejected thread. `None` on a
    /// rejected thread means the draft was declined, and a declined draft posts
    /// nothing.
    pub reply: Option<String>,
}

impl TriageItem {
    fn new(thread: Thread, verdict: Option<Verdict>) -> Self {
        Self {
            thread,
            verdict,
            decision: None,
            edited_prose: None,
            reply: None,
        }
    }

    /// The prose an apply turn should act on: the user's edit if they made one,
    /// otherwise the thread's own text.
    pub fn effective_prose(&self) -> &str {
        self.edited_prose
            .as_deref()
            .unwrap_or(&self.thread.root().message)
    }

    /// Whether this thread carries no decision at all. Counted at the gate.
    pub fn is_undecided(&self) -> bool {
        self.decision.is_none()
    }

    /// Whether this thread still wants the human's attention.
    ///
    /// Deliberately **not** the same as [`is_undecided`](Self::is_undecided): a
    /// deferral *is* a decision, but it means "not yet", so navigation must
    /// come back to it. Conflating the two is what made next-undecided walk
    /// past deferrals and never offer them again.
    pub fn is_pending(&self) -> bool {
        matches!(self.decision, None | Some(Decision::Defer))
    }

    /// Whether a reply should be drafted for it — every rejection, regardless of
    /// what Claude's verdict was. A rejection against an `agree` verdict is
    /// precisely where the human held context Claude lacked.
    pub fn needs_reply(&self) -> bool {
        self.decision == Some(Decision::Reject)
    }
}

/// The triage queue and its cursor.
#[derive(Debug, Clone, PartialEq)]
pub struct Triage {
    items: Vec<TriageItem>,
    cursor: usize,
    /// Threads dropped for patchset age, carried so the count can be reported
    /// before triage begins.
    pub skipped_older_patchsets: usize,
}

impl Triage {
    /// Build a queue from threads and whatever verdicts were produced for them.
    ///
    /// Verdicts are matched by the thread's opening comment id. A thread with no
    /// matching verdict is kept and marked unadjudicated rather than dropped.
    pub fn new(threads: Vec<Thread>, verdicts: &[Verdict], skipped_older_patchsets: usize) -> Self {
        let items = threads
            .into_iter()
            .map(|thread| {
                let verdict = verdicts
                    .iter()
                    .find(|v| v.comment_id == thread.root().id)
                    .cloned();
                TriageItem::new(thread, verdict)
            })
            .collect();
        Self {
            items,
            cursor: 0,
            skipped_older_patchsets,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn items(&self) -> &[TriageItem] {
        &self.items
    }

    pub fn cursor(&self) -> usize {
        self.cursor
    }

    /// The thread under the cursor.
    pub fn current(&self) -> Option<&TriageItem> {
        self.items.get(self.cursor)
    }

    pub fn current_mut(&mut self) -> Option<&mut TriageItem> {
        self.items.get_mut(self.cursor)
    }

    /// Threads whose verdict pass failed, which must be visible as such.
    pub fn unadjudicated(&self) -> usize {
        self.items.iter().filter(|i| i.verdict.is_none()).count()
    }

    /// Threads still carrying no decision.
    pub fn undecided(&self) -> usize {
        self.items.iter().filter(|i| i.is_undecided()).count()
    }

    // --- decisions ---------------------------------------------------------

    /// Record a decision for the current thread. The human's choice governs,
    /// whatever the verdict said.
    pub fn decide(&mut self, decision: Decision) {
        if let Some(item) = self.items.get_mut(self.cursor) {
            item.decision = Some(decision);
        }
    }

    /// Replace the current thread's prose, which is what the apply turn uses.
    pub fn edit_prose(&mut self, prose: impl Into<String>) {
        if let Some(item) = self.items.get_mut(self.cursor) {
            item.edited_prose = Some(prose.into());
        }
    }

    /// Approve a reply for a rejected thread.
    pub fn approve_reply(&mut self, index: usize, reply: impl Into<String>) {
        if let Some(item) = self.items.get_mut(index) {
            item.reply = Some(reply.into());
        }
    }

    /// Indices of threads awaiting a reply decision, in queue order.
    pub fn awaiting_replies(&self) -> Vec<usize> {
        self.items
            .iter()
            .enumerate()
            .filter(|(_, item)| item.needs_reply())
            .map(|(index, _)| index)
            .collect()
    }

    // --- navigation --------------------------------------------------------

    /// Move to the adjacent thread. Stops at the ends; **reaching the last one
    /// does not end triage.**
    pub fn next(&mut self) {
        if self.cursor + 1 < self.items.len() {
            self.cursor += 1;
        }
    }

    pub fn prev(&mut self) {
        self.cursor = self.cursor.saturating_sub(1);
    }

    /// Move to the next thread with no decision, wrapping once.
    ///
    /// Distinct from [`next`](Self::next) because deferral makes the queue
    /// non-linear: after a few deferrals the adjacent thread is usually already
    /// decided, and walking past them by hand is the friction this avoids.
    pub fn next_undecided(&mut self) {
        let len = self.items.len();
        if len == 0 {
            return;
        }
        for step in 1..=len {
            let index = (self.cursor + step) % len;
            // `is_pending`, not `is_undecided`: a deferred thread said "not
            // yet", so this is exactly what must come back to it.
            if self.items[index].is_pending() {
                self.cursor = index;
                return;
            }
        }
    }

    /// Jump to the first thread anchored on `path`, if there is one.
    pub fn jump_to_path(&mut self, path: &str) -> bool {
        let found = self
            .items
            .iter()
            .position(|item| item.thread.anchor.gerrit_path() == path);
        if let Some(index) = found {
            self.cursor = index;
        }
        found.is_some()
    }

    // --- the completion gate ----------------------------------------------

    /// What ending triage right now would leave unsettled.
    ///
    /// Reported at the gate so the deferral-becomes-skipped transition is
    /// stated rather than silent.
    pub fn gate_report(&self) -> GateReport {
        GateReport {
            undecided: self.undecided(),
            deferred: self
                .items
                .iter()
                .filter(|i| i.decision == Some(Decision::Defer))
                .count(),
            unreplied: self
                .items
                .iter()
                .filter(|i| i.needs_reply() && i.reply.is_none())
                .count(),
        }
    }

    /// End triage, mapping every thread onto the fate finalize will post.
    ///
    /// Deferred and undecided threads both become [`Fate::Skipped`]: at the gate
    /// "not yet" becomes "not doing it".
    pub fn finish(self) -> Vec<TriagedThread> {
        self.items
            .into_iter()
            .map(|item| {
                let fate = match item.decision {
                    Some(Decision::Accept) => Fate::Accepted,
                    Some(Decision::FixedByHand) => Fate::FixedByHand,
                    Some(Decision::Reject) => Fate::Rejected { reply: item.reply },
                    Some(Decision::Defer) | None => Fate::Skipped,
                };
                TriagedThread {
                    thread: item.thread,
                    fate,
                }
            })
            .collect()
    }
}

/// What is still unsettled at the completion gate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct GateReport {
    /// Threads with no decision at all.
    pub undecided: usize,
    /// Threads explicitly deferred — "not yet", about to become "not doing it".
    pub deferred: usize,
    /// Rejected threads whose reply has not been approved or declined.
    pub unreplied: usize,
}

impl GateReport {
    /// Whether anything needs stating before the gate closes.
    pub fn is_clean(&self) -> bool {
        self.undecided == 0 && self.deferred == 0 && self.unreplied == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gerrit::Comment;
    use crate::gerrit::thread::assemble;

    fn thread_on(path: &str, id: &str) -> Thread {
        let comment: Comment = serde_json::from_value(serde_json::json!({
            "id": id, "unresolved": true, "patch_set": 3, "line": 1,
            "message": format!("comment {id}"),
        }))
        .unwrap();
        assemble(path, vec![comment]).remove(0)
    }

    fn verdict_for(id: &str) -> Verdict {
        serde_json::from_value(serde_json::json!({
            "comment_id": id, "verdict": "agree",
            "justification": "Real issue.", "depends_on": null,
        }))
        .unwrap()
    }

    fn triage_of(ids: &[&str]) -> Triage {
        let threads: Vec<Thread> = ids.iter().map(|id| thread_on("a.rs", id)).collect();
        let verdicts: Vec<Verdict> = ids.iter().map(|id| verdict_for(id)).collect();
        Triage::new(threads, &verdicts, 0)
    }

    #[test]
    fn verdicts_are_matched_to_their_threads() {
        let triage = triage_of(&["c1", "c2"]);
        assert_eq!(triage.len(), 2);
        assert_eq!(
            triage
                .current()
                .unwrap()
                .verdict
                .as_ref()
                .unwrap()
                .comment_id,
            "c1"
        );
    }

    /// A missing verdict must stay visibly missing — the same failure mode the
    /// required `depends_on` exists to prevent, one level up.
    #[test]
    fn a_thread_without_a_verdict_is_kept_and_counted_as_unadjudicated() {
        let triage = Triage::new(vec![thread_on("a.rs", "c1")], &[], 0);
        assert_eq!(triage.len(), 1, "kept, not dropped");
        assert!(triage.current().unwrap().verdict.is_none());
        assert_eq!(triage.unadjudicated(), 1);
    }

    #[test]
    fn the_human_decision_governs_whatever_the_verdict_said() {
        let mut triage = triage_of(&["c1"]);
        assert_eq!(
            triage.current().unwrap().verdict.as_ref().unwrap().verdict,
            crate::verdict::Adjudication::Agree
        );
        triage.decide(Decision::Reject);
        let fates = triage.finish();
        assert!(matches!(fates[0].fate, Fate::Rejected { .. }));
    }

    // --- navigation --------------------------------------------------------

    /// Navigating past the last comment must not end triage — the gate does.
    #[test]
    fn next_stops_at_the_last_thread() {
        let mut triage = triage_of(&["c1", "c2"]);
        triage.next();
        triage.next();
        triage.next();
        assert_eq!(triage.cursor(), 1, "clamped, not wrapped and not ended");
    }

    #[test]
    fn prev_stops_at_the_first_thread() {
        let mut triage = triage_of(&["c1", "c2"]);
        triage.prev();
        assert_eq!(triage.cursor(), 0);
    }

    /// The reason next-undecided is a separate motion: after deferrals the
    /// adjacent thread is usually already decided.
    #[test]
    fn next_undecided_skips_decided_threads_and_wraps() {
        let mut triage = triage_of(&["c1", "c2", "c3"]);
        triage.decide(Decision::Accept); // c1 decided
        triage.next();
        triage.decide(Decision::Accept); // c2 decided
        triage.cursor = 0;

        triage.next_undecided();
        assert_eq!(triage.cursor(), 2, "skipped the decided c2");

        // Only c3 is undecided, and the cursor is on it: wrapping finds itself.
        triage.next_undecided();
        assert_eq!(triage.cursor(), 2);
    }

    #[test]
    fn a_deferred_thread_is_offered_again_by_next_undecided() {
        let mut triage = triage_of(&["c1", "c2"]);
        triage.decide(Decision::Defer);
        triage.next();
        triage.decide(Decision::Accept);

        triage.next_undecided();
        assert_eq!(
            triage.cursor(),
            0,
            "a deferral is a decision for the queue but not for the gate"
        );
    }

    #[test]
    fn next_undecided_on_a_fully_decided_queue_stays_put() {
        let mut triage = triage_of(&["c1"]);
        triage.decide(Decision::Accept);
        triage.next_undecided();
        assert_eq!(triage.cursor(), 0);
    }

    #[test]
    fn jumping_to_a_path_moves_the_cursor() {
        let threads = vec![thread_on("a.rs", "c1"), thread_on("b.rs", "c2")];
        let mut triage = Triage::new(threads, &[], 0);
        assert!(triage.jump_to_path("b.rs"));
        assert_eq!(triage.cursor(), 1);
        assert!(!triage.jump_to_path("nope.rs"));
        assert_eq!(triage.cursor(), 1, "a miss leaves the cursor alone");
    }

    // --- prose and replies -------------------------------------------------

    #[test]
    fn edited_prose_replaces_the_comment_for_the_apply_turn() {
        let mut triage = triage_of(&["c1"]);
        assert_eq!(triage.current().unwrap().effective_prose(), "comment c1");
        triage.edit_prose("Only rename the variable, do not refactor.");
        assert_eq!(
            triage.current().unwrap().effective_prose(),
            "Only rename the variable, do not refactor."
        );
    }

    #[test]
    fn every_rejection_awaits_a_reply_regardless_of_the_verdict() {
        let mut triage = triage_of(&["c1", "c2"]);
        triage.decide(Decision::Reject); // rejected against an `agree` verdict
        triage.next();
        triage.decide(Decision::Accept);
        assert_eq!(triage.awaiting_replies(), vec![0]);
    }

    #[test]
    fn an_approved_reply_reaches_the_fate() {
        let mut triage = triage_of(&["c1"]);
        triage.decide(Decision::Reject);
        triage.approve_reply(0, "Bounded at 8 items.");
        let fates = triage.finish();
        assert_eq!(
            fates[0].fate,
            Fate::Rejected {
                reply: Some("Bounded at 8 items.".into())
            }
        );
    }

    #[test]
    fn a_declined_draft_leaves_the_rejection_without_a_reply() {
        let mut triage = triage_of(&["c1"]);
        triage.decide(Decision::Reject);
        let fates = triage.finish();
        assert_eq!(fates[0].fate, Fate::Rejected { reply: None });
    }

    // --- the gate ----------------------------------------------------------

    #[test]
    fn the_gate_reports_what_is_still_unsettled() {
        let mut triage = triage_of(&["c1", "c2", "c3"]);
        triage.decide(Decision::Defer);
        triage.next();
        triage.decide(Decision::Reject); // rejected, no reply approved yet
        // c3 left entirely undecided.

        let report = triage.gate_report();
        assert_eq!(report.undecided, 1);
        assert_eq!(report.deferred, 1);
        assert_eq!(report.unreplied, 1);
        assert!(!report.is_clean());
    }

    #[test]
    fn a_fully_settled_queue_reports_a_clean_gate() {
        let mut triage = triage_of(&["c1"]);
        triage.decide(Decision::Accept);
        assert!(triage.gate_report().is_clean());
    }

    /// At the gate, "not yet" becomes "not doing it" — for deferrals and for
    /// threads never looked at.
    #[test]
    fn deferred_and_undecided_both_become_skipped() {
        let mut triage = triage_of(&["c1", "c2"]);
        triage.decide(Decision::Defer);
        let fates = triage.finish();
        assert_eq!(fates[0].fate, Fate::Skipped, "deferred");
        assert_eq!(fates[1].fate, Fate::Skipped, "never decided");
    }

    #[test]
    fn each_decision_maps_to_its_fate() {
        let mut triage = triage_of(&["c1", "c2", "c3", "c4"]);
        triage.decide(Decision::Accept);
        triage.next();
        triage.decide(Decision::FixedByHand);
        triage.next();
        triage.decide(Decision::Reject);
        triage.approve_reply(2, "No.");
        triage.next();
        triage.decide(Decision::Defer);

        let fates = triage.finish();
        assert_eq!(fates[0].fate, Fate::Accepted);
        assert_eq!(fates[1].fate, Fate::FixedByHand);
        assert_eq!(
            fates[2].fate,
            Fate::Rejected {
                reply: Some("No.".into())
            }
        );
        assert_eq!(fates[3].fate, Fate::Skipped);
    }

    #[test]
    fn the_patchset_skip_count_is_carried_for_reporting() {
        let triage = Triage::new(vec![thread_on("a.rs", "c1")], &[], 3);
        assert_eq!(triage.skipped_older_patchsets, 3);
    }

    #[test]
    fn an_empty_queue_gates_cleanly_and_finishes_empty() {
        let triage = Triage::new(Vec::new(), &[], 0);
        assert!(triage.is_empty());
        assert!(triage.gate_report().is_clean());
        assert!(triage.finish().is_empty());
    }
}
