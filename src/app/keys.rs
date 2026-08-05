//! Key events to actions.
//!
//! Deliberately a **pure function**: no state, no side effects, no terminal. It
//! is the one part of the UI layer that `cargo test` can pin completely, so
//! every binding in `tasks.md` 5.5 is covered by a test rather than by trying
//! it in a terminal.
//!
//! Dispatch depends on the mode because a printable key means two different
//! things: `a` accepts a thread during triage, and types an `a` while editing
//! prose. Getting that wrong would silently accept threads as someone typed.

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::{Action, Mode};
use crate::triage::Decision;

/// The action a key produces in a given mode, or `None` if it is unbound.
pub fn action_for(key: KeyEvent, mode: Mode) -> Option<Action> {
    // Ctrl-C always quits, in every mode. A user reaching for it wants out,
    // not a literal control character in their comment prose.
    if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
        return Some(Action::Quit);
    }
    match mode {
        Mode::Triage => triage_action(key),
        Mode::Prose | Mode::Reply => text_action(key),
        Mode::Gate => gate_action(key),
    }
}

/// Bindings while deciding on threads.
fn triage_action(key: KeyEvent) -> Option<Action> {
    Some(match key.code {
        KeyCode::Char('a') => Action::Decide(Decision::Accept),
        KeyCode::Char('r') => Action::Decide(Decision::Reject),
        KeyCode::Char('d') => Action::Decide(Decision::Defer),
        KeyCode::Char('h') => Action::Decide(Decision::FixedByHand),
        KeyCode::Char('e') => Action::BeginProseEdit,

        KeyCode::Char('n') | KeyCode::Down => Action::Next,
        KeyCode::Char('p') | KeyCode::Up => Action::Prev,
        KeyCode::Tab => Action::NextUndecided,

        KeyCode::Char('t') => Action::ToggleTree,
        KeyCode::Char('w') => Action::ShowWorktreePath,

        KeyCode::PageDown | KeyCode::Char(' ') => Action::ScrollDocDown,
        KeyCode::PageUp => Action::ScrollDocUp,

        // Ending triage is deliberate and uppercase: it is the gate, and it
        // should not be one keystroke away from `e` or `d`.
        KeyCode::Char('F') => Action::OpenGate,
        KeyCode::Char('q') => Action::Quit,
        _ => return None,
    })
}

/// Bindings while typing into the minibuffer.
///
/// Printable characters are text here, never commands — which is the whole
/// reason mode exists.
fn text_action(key: KeyEvent) -> Option<Action> {
    Some(match key.code {
        KeyCode::Char(c) => Action::Input(c),
        KeyCode::Backspace => Action::Backspace,
        KeyCode::Enter => Action::Confirm,
        KeyCode::Esc => Action::Cancel,
        _ => return None,
    })
}

/// Bindings at the completion gate, which is a yes/no question.
fn gate_action(key: KeyEvent) -> Option<Action> {
    Some(match key.code {
        KeyCode::Char('y') | KeyCode::Enter => Action::Confirm,
        KeyCode::Char('n') | KeyCode::Esc => Action::Cancel,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn ch(c: char) -> KeyEvent {
        key(KeyCode::Char(c))
    }

    fn ctrl(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
    }

    #[test]
    fn every_decision_has_a_binding() {
        for (c, decision) in [
            ('a', Decision::Accept),
            ('r', Decision::Reject),
            ('d', Decision::Defer),
            ('h', Decision::FixedByHand),
        ] {
            assert_eq!(
                action_for(ch(c), Mode::Triage),
                Some(Action::Decide(decision)),
                "`{c}` should decide"
            );
        }
    }

    #[test]
    fn navigation_has_both_letter_and_arrow_bindings() {
        assert_eq!(action_for(ch('n'), Mode::Triage), Some(Action::Next));
        assert_eq!(
            action_for(key(KeyCode::Down), Mode::Triage),
            Some(Action::Next)
        );
        assert_eq!(action_for(ch('p'), Mode::Triage), Some(Action::Prev));
        assert_eq!(
            action_for(key(KeyCode::Up), Mode::Triage),
            Some(Action::Prev)
        );
        assert_eq!(
            action_for(key(KeyCode::Tab), Mode::Triage),
            Some(Action::NextUndecided)
        );
    }

    #[test]
    fn the_remaining_5_5_bindings_are_present() {
        assert_eq!(
            action_for(ch('e'), Mode::Triage),
            Some(Action::BeginProseEdit)
        );
        assert_eq!(action_for(ch('t'), Mode::Triage), Some(Action::ToggleTree));
        assert_eq!(
            action_for(ch('w'), Mode::Triage),
            Some(Action::ShowWorktreePath)
        );
    }

    /// The bug this mode split exists to prevent: typing a comment must not
    /// silently accept and reject threads.
    #[test]
    fn printable_keys_are_text_while_editing_not_commands() {
        for c in ['a', 'r', 'd', 'q', 'F'] {
            assert_eq!(
                action_for(ch(c), Mode::Prose),
                Some(Action::Input(c)),
                "`{c}` must be text in prose mode"
            );
            assert_eq!(action_for(ch(c), Mode::Reply), Some(Action::Input(c)));
        }
    }

    #[test]
    fn text_mode_submits_and_cancels() {
        assert_eq!(
            action_for(key(KeyCode::Enter), Mode::Prose),
            Some(Action::Confirm)
        );
        assert_eq!(
            action_for(key(KeyCode::Esc), Mode::Prose),
            Some(Action::Cancel)
        );
        assert_eq!(
            action_for(key(KeyCode::Backspace), Mode::Prose),
            Some(Action::Backspace)
        );
    }

    /// Ending triage is a gate, not a keystroke away from a decision.
    #[test]
    fn ending_triage_is_an_uppercase_deliberate_key() {
        assert_eq!(action_for(ch('F'), Mode::Triage), Some(Action::OpenGate));
        assert_eq!(
            action_for(ch('f'), Mode::Triage),
            None,
            "lowercase must not end triage by accident"
        );
    }

    #[test]
    fn the_gate_is_a_yes_no_question() {
        assert_eq!(action_for(ch('y'), Mode::Gate), Some(Action::Confirm));
        assert_eq!(
            action_for(key(KeyCode::Enter), Mode::Gate),
            Some(Action::Confirm)
        );
        assert_eq!(action_for(ch('n'), Mode::Gate), Some(Action::Cancel));
        assert_eq!(
            action_for(key(KeyCode::Esc), Mode::Gate),
            Some(Action::Cancel)
        );
    }

    /// `n` means "next" during triage and "no" at the gate — the same key, two
    /// meanings, which is only safe because the gate is a distinct mode.
    #[test]
    fn n_means_next_in_triage_but_no_at_the_gate() {
        assert_eq!(action_for(ch('n'), Mode::Triage), Some(Action::Next));
        assert_eq!(action_for(ch('n'), Mode::Gate), Some(Action::Cancel));
    }

    #[test]
    fn ctrl_c_quits_from_every_mode() {
        for mode in [Mode::Triage, Mode::Prose, Mode::Reply, Mode::Gate] {
            assert_eq!(
                action_for(ctrl('c'), mode),
                Some(Action::Quit),
                "ctrl-c must escape {mode:?}"
            );
        }
    }

    #[test]
    fn unbound_keys_produce_nothing() {
        assert_eq!(action_for(key(KeyCode::F(5)), Mode::Triage), None);
        assert_eq!(action_for(key(KeyCode::Home), Mode::Gate), None);
    }
}
