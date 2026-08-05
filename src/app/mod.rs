//! The triage application: state, modes, and the action it takes on each key.
//!
//! Split three ways on purpose, because only two of the three can be tested:
//!
//! ```text
//!   keys.rs      KeyEvent -> Action     pure, fully tested
//!   mod.rs       Action   -> state      pure, fully tested
//!   render.rs    state    -> screen     needs eyes on a terminal
//! ```
//!
//! Everything that decides *what happens* lives on this side of the line, so
//! the untestable surface is composition and colour rather than behaviour.

pub mod keys;
pub mod render;
pub mod terminal;
pub mod tree;

pub use keys::action_for;
pub use render::draw;
pub use tree::TreeEntry;

use crate::gerrit::ChangeInfo;
use crate::minibuffer::Minibuffer;
use crate::submit::TriagedThread;
use crate::triage::{Decision, GateReport, SharedDependency, Triage, collate};

/// What the keyboard currently means.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Deciding on threads.
    Triage,
    /// Editing a comment's prose before it feeds an apply turn.
    Prose,
    /// Editing or approving a drafted reply to a rejected thread.
    Reply,
    /// Answering the completion gate.
    Gate,
}

/// Something the user asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Decide(Decision),
    Next,
    Prev,
    NextUndecided,
    ToggleTree,
    BeginProseEdit,
    ShowWorktreePath,
    ScrollDocUp,
    ScrollDocDown,
    /// Ask to end triage — opens the gate, does not close it.
    OpenGate,
    /// Submit the minibuffer, or answer the gate yes.
    Confirm,
    /// Abandon the minibuffer, or answer the gate no.
    Cancel,
    Input(char),
    Backspace,
    Quit,
}

/// Why the application stopped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// Still running.
    Running,
    /// Triage completed through the gate; these fates go to apply and finalize.
    Finished(Vec<TriagedThread>),
    /// The user aborted. The worktree and any confirmed edits stay put.
    Aborted,
}

/// How long a frame waits for input before redrawing anyway.
///
/// The redraw rebuilds the document from the worktree, so an edit made in the
/// user's own editor appears within this window without any file watching
/// (`tasks.md` 5.10). The pane is read-only, so there is no in-application
/// buffer to reconcile and a reload can never prompt about a conflict.
const FRAME_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(250);

/// Run the triage UI until the user finishes or aborts.
///
/// Takes over the terminal and always hands it back — on a normal exit, on an
/// error, and on a panic (see [`terminal`]).
pub fn run(
    app: &mut App,
    files: &impl crate::apply::WorktreeFiles,
    commit_message: &str,
    theme: &crate::theme::Theme,
) -> std::io::Result<Outcome> {
    use ratatui::crossterm::event::{self, Event};

    let mut tui = terminal::init()?;
    let result = (|| -> std::io::Result<Outcome> {
        while app.is_running() {
            // Rebuilt each frame from the worktree, which is what picks up an
            // external edit.
            let document = app.current_document(commit_message, files);
            tui.draw(|frame| render::draw(frame, app, &document, theme))?;

            if !event::poll(FRAME_TIMEOUT)? {
                continue;
            }
            if let Event::Key(key) = event::read()?
                && key.is_press()
                && let Some(action) = action_for(key, app.mode)
            {
                app.handle(action);
            }
        }
        Ok(app.outcome().clone())
    })();
    terminal::restore()?;
    result
}

/// The triage application's state.
#[derive(Debug)]
pub struct App {
    pub triage: Triage,
    pub change: ChangeInfo,
    pub mode: Mode,
    /// The file tree is toggleable, so a narrow terminal can give its width to
    /// the document.
    pub tree_visible: bool,
    /// The active prompt, when one is open.
    pub minibuffer: Option<Minibuffer>,
    /// A one-line message for the status bar.
    pub status: Option<String>,
    /// Where the worktree is, for the fix-in-your-own-editor path.
    pub worktree: String,
    /// Vertical scroll of the document pane.
    pub doc_scroll: usize,
    /// The gate's report while it is open.
    pub gate: Option<GateReport>,
    /// Which rejected thread's reply is being edited, while in [`Mode::Reply`].
    reply_target: Option<usize>,
    outcome: Outcome,
}

impl App {
    pub fn new(triage: Triage, change: ChangeInfo, worktree: impl Into<String>) -> Self {
        let mut app = Self {
            triage,
            change,
            mode: Mode::Triage,
            tree_visible: true,
            minibuffer: None,
            status: None,
            worktree: worktree.into(),
            doc_scroll: 0,
            gate: None,
            reply_target: None,
            outcome: Outcome::Running,
        };
        app.status = app.load_notice();
        app
    }

    /// What to say before triage begins.
    ///
    /// A queue shorter than the change's unresolved count must not read as a
    /// change with fewer comments, so the skipped count is stated up front
    /// (`tasks.md` 5.1b). Unadjudicated threads are stated for the same reason.
    fn load_notice(&self) -> Option<String> {
        let mut parts = Vec::new();
        if self.triage.skipped_older_patchsets > 0 {
            parts.push(format!(
                "{} thread(s) on earlier patchsets were NOT triaged",
                self.triage.skipped_older_patchsets
            ));
        }
        if self.triage.unadjudicated() > 0 {
            parts.push(format!(
                "{} thread(s) have no verdict",
                self.triage.unadjudicated()
            ));
        }
        (!parts.is_empty()).then(|| parts.join("; "))
    }

    /// Whether the loop should stop, and why.
    pub fn outcome(&self) -> &Outcome {
        &self.outcome
    }

    pub fn is_running(&self) -> bool {
        self.outcome == Outcome::Running
    }

    /// The document the thread under the cursor is anchored to.
    ///
    /// Rebuilt on demand rather than cached, so a file edited in the user's own
    /// editor is picked up on the next redraw.
    pub fn current_document(
        &self,
        commit_message: &str,
        files: &impl crate::apply::WorktreeFiles,
    ) -> crate::triage::Document {
        match self.triage.current() {
            Some(item) => crate::triage::Document::for_thread(
                &item.thread,
                &self.change,
                commit_message,
                files,
            ),
            None => crate::triage::Document {
                title: "no threads".to_string(),
                lines: vec!["Nothing to triage on the current patchset.".to_string()],
                anchor_line: None,
                highlighted: false,
            },
        }
    }

    /// The out-of-code facts the current verdicts rest on, collapsed so a fact
    /// covering three verdicts is shown once.
    pub fn shared_dependencies(&self) -> Vec<SharedDependency> {
        let verdicts: Vec<_> = self
            .triage
            .items()
            .iter()
            .filter_map(|item| item.verdict.clone())
            .collect();
        collate(&verdicts)
    }

    /// Apply one action.
    pub fn handle(&mut self, action: Action) {
        self.status = None;
        match action {
            Action::Quit => self.outcome = Outcome::Aborted,
            Action::Decide(decision) => self.decide(decision),
            Action::Next => self.move_cursor(|t| t.next()),
            Action::Prev => self.move_cursor(|t| t.prev()),
            Action::NextUndecided => self.move_cursor(|t| t.next_undecided()),
            Action::ToggleTree => self.tree_visible = !self.tree_visible,
            Action::ShowWorktreePath => {
                self.status = Some(format!("worktree: {}", self.worktree));
            }
            Action::ScrollDocDown => self.doc_scroll += 1,
            Action::ScrollDocUp => self.doc_scroll = self.doc_scroll.saturating_sub(1),
            Action::BeginProseEdit => self.begin_prose_edit(),
            Action::OpenGate => self.open_gate(),
            Action::Input(c) => self.type_char(c),
            Action::Backspace => self.backspace(),
            Action::Confirm => self.confirm(),
            Action::Cancel => self.cancel(),
        }
    }

    /// Record a decision, then move on to the next thread wanting attention.
    ///
    /// Advancing after a decision is what keeps triage flowing; it is *not* the
    /// same as ending triage, which only the gate does.
    fn decide(&mut self, decision: Decision) {
        self.triage.decide(decision);
        self.triage.next_undecided();
        self.doc_scroll = 0;
    }

    fn move_cursor(&mut self, movement: impl FnOnce(&mut Triage)) {
        movement(&mut self.triage);
        self.doc_scroll = 0;
    }

    fn begin_prose_edit(&mut self) {
        let Some(item) = self.triage.current() else {
            return;
        };
        self.minibuffer = Some(Minibuffer::editing(
            "Comment: ",
            item.effective_prose().to_string(),
        ));
        self.mode = Mode::Prose;
    }

    /// Open the completion gate. It reports and asks; it does not end anything.
    fn open_gate(&mut self) {
        self.gate = Some(self.triage.gate_report());
        self.mode = Mode::Gate;
    }

    fn type_char(&mut self, c: char) {
        if let Some(mini) = self.minibuffer.as_mut() {
            mini.push(c);
        }
    }

    fn backspace(&mut self) {
        if let Some(mini) = self.minibuffer.as_mut() {
            mini.backspace();
        }
    }

    /// Submit whatever is open.
    fn confirm(&mut self) {
        match self.mode {
            Mode::Prose => self.submit_prose(),
            Mode::Reply => self.submit_reply(),
            Mode::Gate => self.close_gate(),
            Mode::Triage => {}
        }
    }

    fn submit_prose(&mut self) {
        if let Some(mini) = self.minibuffer.take()
            && !mini.is_empty()
        {
            self.triage.edit_prose(mini.input);
        }
        self.mode = Mode::Triage;
    }

    /// Answer the reply under edit.
    ///
    /// Empty text **declines** the draft rather than leaving it unanswered.
    /// Those are different states: an unanswered reply keeps the pass prompting,
    /// and conflating them re-offered a declined draft forever.
    fn submit_reply(&mut self) {
        if let (Some(mini), Some(index)) = (self.minibuffer.take(), self.reply_target) {
            if mini.is_empty() {
                self.triage.decline_reply(index);
            } else {
                self.triage.approve_reply(index, mini.input);
            }
        }
        self.reply_target = None;
        self.next_reply();
    }

    /// Move to the next rejected thread still awaiting a reply, or back to
    /// triage when there are none.
    fn next_reply(&mut self) {
        let pending: Vec<usize> = self
            .triage
            .awaiting_replies()
            .into_iter()
            .filter(|index| self.triage.items()[*index].reply.is_pending())
            .collect();
        match pending.first() {
            Some(&index) => {
                self.reply_target = Some(index);
                self.minibuffer = Some(Minibuffer::new("Reply: "));
                self.mode = Mode::Reply;
            }
            None => {
                self.reply_target = None;
                self.minibuffer = None;
                self.mode = Mode::Triage;
                self.open_gate();
            }
        }
    }

    /// Answer the gate yes.
    ///
    /// Rejected threads still owing a reply divert into the reply pass first,
    /// so finalize is unattended: it posts only drafts already approved.
    fn close_gate(&mut self) {
        let report = self.triage.gate_report();
        if report.unreplied > 0 {
            self.gate = None;
            self.next_reply();
            return;
        }
        self.gate = None;
        let triage = std::mem::replace(&mut self.triage, Triage::new(Vec::new(), &[], 0));
        self.outcome = Outcome::Finished(triage.finish());
    }

    /// Back out of whatever is open, without deciding anything.
    fn cancel(&mut self) {
        self.minibuffer = None;
        self.reply_target = None;
        self.gate = None;
        self.mode = Mode::Triage;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gerrit::thread::assemble;
    use crate::gerrit::{Comment, Thread};
    use crate::submit::Fate;

    fn thread_on(path: &str, id: &str) -> Thread {
        let comment: Comment = serde_json::from_value(serde_json::json!({
            "id": id, "unresolved": true, "patch_set": 3, "line": 1,
            "message": format!("comment {id}"),
        }))
        .unwrap();
        assemble(path, vec![comment]).remove(0)
    }

    fn change() -> ChangeInfo {
        serde_json::from_value(serde_json::json!({
            "id": "12345", "project": "proj", "branch": "main",
            "subject": "s", "current_revision": "sha3",
        }))
        .unwrap()
    }

    fn app_with(ids: &[&str]) -> App {
        let threads: Vec<Thread> = ids.iter().map(|id| thread_on("a.rs", id)).collect();
        App::new(Triage::new(threads, &[], 0), change(), "/wt")
    }

    #[test]
    fn deciding_advances_to_the_next_thread_wanting_attention() {
        let mut app = app_with(&["c1", "c2"]);
        app.handle(Action::Decide(Decision::Accept));
        assert_eq!(app.triage.cursor(), 1, "flow continues");
        assert!(app.is_running(), "but triage has not ended");
    }

    /// Reaching the end must not finish anything — only the gate does.
    #[test]
    fn deciding_the_last_thread_does_not_end_triage() {
        let mut app = app_with(&["c1"]);
        app.handle(Action::Decide(Decision::Accept));
        assert!(app.is_running());
        assert_eq!(app.mode, Mode::Triage);
    }

    #[test]
    fn the_tree_toggles() {
        let mut app = app_with(&["c1"]);
        assert!(app.tree_visible);
        app.handle(Action::ToggleTree);
        assert!(!app.tree_visible);
        app.handle(Action::ToggleTree);
        assert!(app.tree_visible);
    }

    #[test]
    fn the_worktree_path_is_obtainable_mid_review() {
        let mut app = app_with(&["c1"]);
        app.handle(Action::ShowWorktreePath);
        assert!(app.status.as_ref().unwrap().contains("/wt"));
    }

    #[test]
    fn scrolling_clamps_at_the_top() {
        let mut app = app_with(&["c1"]);
        app.handle(Action::ScrollDocUp);
        assert_eq!(app.doc_scroll, 0);
        app.handle(Action::ScrollDocDown);
        assert_eq!(app.doc_scroll, 1);
    }

    #[test]
    fn moving_between_threads_resets_the_document_scroll() {
        let mut app = app_with(&["c1", "c2"]);
        app.handle(Action::ScrollDocDown);
        app.handle(Action::Next);
        assert_eq!(app.doc_scroll, 0);
    }

    // --- prose editing -----------------------------------------------------

    #[test]
    fn editing_prose_starts_from_the_existing_comment() {
        let mut app = app_with(&["c1"]);
        app.handle(Action::BeginProseEdit);
        assert_eq!(app.mode, Mode::Prose);
        assert_eq!(app.minibuffer.as_ref().unwrap().input, "comment c1");
    }

    #[test]
    fn submitted_prose_becomes_what_the_apply_turn_uses() {
        let mut app = app_with(&["c1"]);
        app.handle(Action::BeginProseEdit);
        for _ in 0.."comment c1".len() {
            app.handle(Action::Backspace);
        }
        for c in "rename only".chars() {
            app.handle(Action::Input(c));
        }
        app.handle(Action::Confirm);

        assert_eq!(app.mode, Mode::Triage);
        assert_eq!(
            app.triage.current().unwrap().effective_prose(),
            "rename only"
        );
    }

    #[test]
    fn cancelling_prose_leaves_the_comment_alone() {
        let mut app = app_with(&["c1"]);
        app.handle(Action::BeginProseEdit);
        app.handle(Action::Input('x'));
        app.handle(Action::Cancel);
        assert_eq!(app.mode, Mode::Triage);
        assert_eq!(
            app.triage.current().unwrap().effective_prose(),
            "comment c1"
        );
    }

    // --- the gate and the reply pass ---------------------------------------

    #[test]
    fn the_gate_reports_before_it_closes() {
        let mut app = app_with(&["c1", "c2"]);
        app.handle(Action::OpenGate);
        assert_eq!(app.mode, Mode::Gate);
        assert_eq!(app.gate.unwrap().undecided, 2);
        assert!(app.is_running(), "opening the gate decides nothing");
    }

    #[test]
    fn answering_the_gate_no_returns_to_triage_unchanged() {
        let mut app = app_with(&["c1"]);
        app.handle(Action::OpenGate);
        app.handle(Action::Cancel);
        assert_eq!(app.mode, Mode::Triage);
        assert!(app.is_running());
    }

    #[test]
    fn answering_the_gate_yes_finishes_with_fates() {
        let mut app = app_with(&["c1", "c2"]);
        app.handle(Action::Decide(Decision::Accept));
        app.handle(Action::Decide(Decision::Defer));
        app.handle(Action::OpenGate);
        app.handle(Action::Confirm);

        match app.outcome() {
            Outcome::Finished(fates) => {
                assert_eq!(fates[0].fate, Fate::Accepted);
                assert_eq!(fates[1].fate, Fate::Skipped, "deferred becomes skipped");
            }
            other => panic!("expected Finished, got {other:?}"),
        }
    }

    /// Finalize must be unattended, so an unanswered reply diverts the gate
    /// into the reply pass rather than closing over it.
    #[test]
    fn a_rejection_without_a_reply_diverts_the_gate_into_the_reply_pass() {
        let mut app = app_with(&["c1"]);
        app.handle(Action::Decide(Decision::Reject));
        app.handle(Action::OpenGate);
        app.handle(Action::Confirm);

        assert_eq!(app.mode, Mode::Reply, "not finished — a reply is owed");
        assert!(app.is_running());
        assert!(app.minibuffer.is_some());
    }

    #[test]
    fn approving_a_reply_carries_it_to_the_fate() {
        let mut app = app_with(&["c1"]);
        app.handle(Action::Decide(Decision::Reject));
        app.handle(Action::OpenGate);
        app.handle(Action::Confirm); // diverts into the reply pass
        for c in "Bounded at 8.".chars() {
            app.handle(Action::Input(c));
        }
        app.handle(Action::Confirm); // submit the reply -> gate reopens
        app.handle(Action::Confirm); // answer the gate

        match app.outcome() {
            Outcome::Finished(fates) => assert_eq!(
                fates[0].fate,
                Fate::Rejected {
                    reply: Some("Bounded at 8.".into())
                }
            ),
            other => panic!("expected Finished, got {other:?}"),
        }
    }

    #[test]
    fn an_empty_reply_is_a_declined_draft_and_posts_nothing() {
        let mut app = app_with(&["c1"]);
        app.handle(Action::Decide(Decision::Reject));
        app.handle(Action::OpenGate);
        app.handle(Action::Confirm);
        app.handle(Action::Confirm); // submit nothing
        app.handle(Action::Confirm); // answer the gate

        match app.outcome() {
            Outcome::Finished(fates) => {
                assert_eq!(fates[0].fate, Fate::Rejected { reply: None })
            }
            other => panic!("expected Finished, got {other:?}"),
        }
    }

    /// Declining a draft must SETTLE it. When "declined" and "not answered"
    /// were the same state, the reply pass re-offered the same thread forever
    /// and triage could never end.
    #[test]
    fn a_declined_draft_is_not_offered_again() {
        let mut app = app_with(&["c1"]);
        app.handle(Action::Decide(Decision::Reject));
        app.handle(Action::OpenGate);
        app.handle(Action::Confirm); // into the reply pass
        assert_eq!(app.mode, Mode::Reply);

        app.handle(Action::Confirm); // decline it
        assert_eq!(
            app.mode,
            Mode::Gate,
            "a declined draft settles; it must not re-prompt"
        );
        assert_eq!(app.triage.gate_report().unreplied, 0);
    }

    #[test]
    fn several_rejections_are_replied_to_one_after_another() {
        let mut app = app_with(&["c1", "c2"]);
        app.handle(Action::Decide(Decision::Reject));
        app.handle(Action::Decide(Decision::Reject));
        app.handle(Action::OpenGate);
        app.handle(Action::Confirm);

        assert_eq!(app.mode, Mode::Reply);
        app.handle(Action::Input('a'));
        app.handle(Action::Confirm);
        assert_eq!(app.mode, Mode::Reply, "the second rejection still owes one");
        app.handle(Action::Input('b'));
        app.handle(Action::Confirm);
        assert_eq!(app.mode, Mode::Gate, "both answered, the gate returns");
    }

    // --- quitting ----------------------------------------------------------

    #[test]
    fn quitting_aborts_without_fates() {
        let mut app = app_with(&["c1"]);
        app.handle(Action::Quit);
        assert_eq!(*app.outcome(), Outcome::Aborted);
        assert!(!app.is_running());
    }

    // --- load notice -------------------------------------------------------

    #[test]
    fn skipped_and_unadjudicated_counts_are_stated_up_front() {
        let app = App::new(
            Triage::new(vec![thread_on("a.rs", "c1")], &[], 3),
            change(),
            "/wt",
        );
        let notice = app.status.expect("a notice");
        assert!(notice.contains("3 thread(s) on earlier patchsets"));
        assert!(notice.contains("1 thread(s) have no verdict"));
    }

    #[test]
    fn a_clean_load_says_nothing() {
        let verdict: crate::verdict::Verdict = serde_json::from_value(serde_json::json!({
            "comment_id": "c1", "verdict": "agree",
            "justification": "…", "depends_on": null,
        }))
        .unwrap();
        let app = App::new(
            Triage::new(vec![thread_on("a.rs", "c1")], &[verdict], 0),
            change(),
            "/wt",
        );
        assert_eq!(app.status, None);
    }
}
