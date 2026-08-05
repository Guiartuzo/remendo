//! Driving Claude Code as a subprocess, behind a project-owned trait.
//!
//! **Only the trait, the envelope and the fake live here.** The real
//! implementation — spawning `claude` — is `tasks.md` §4, which is gated behind
//! task 4.11: every flag and response shape was probed against `claude` 2.1.220
//! and the box now runs 2.1.222, so the CLI surface must be re-verified before
//! anything talks to it for real.
//!
//! The trait itself is not gated. Its shape follows from decisions already made
//! (design.md §13/§14), and §6 needs something to apply edits *through* in order
//! to be testable at all.

pub mod fake;

pub use fake::FakeDriver;

use std::path::Path;

/// A Claude session id. One per change, resumed by every later turn so the
/// expensive context is established once (specs/claude-driver).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionId(pub String);

impl SessionId {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// The CLI's result envelope.
///
/// `--output-format json` returns **this**, not the requested payload — parsing
/// its output directly into a verdict cannot succeed (dry-run finding #2). Every
/// trait method returns the envelope rather than the payload so a caller cannot
/// skip `is_error` or drop `total_cost_usd` on the way past.
#[derive(Debug, Clone, PartialEq)]
pub struct Envelope<T> {
    pub is_error: bool,
    /// What this turn cost. Accumulated per change, which is what settles
    /// `open-decisions.md` Tier 5 on the first real run.
    pub total_cost_usd: f64,
    /// The schema-conformant payload, absent on a failed turn.
    pub structured_output: Option<T>,
}

impl<T> Envelope<T> {
    /// The payload, or an error naming what the turn was doing.
    pub fn payload(self, turn: &str) -> Result<T, DriverError> {
        if self.is_error {
            return Err(DriverError::TurnFailed {
                turn: turn.to_string(),
            });
        }
        self.structured_output
            .ok_or_else(|| DriverError::NoStructuredOutput {
                turn: turn.to_string(),
            })
    }
}

/// The Claude turns Remendo issues.
///
/// Apply turns mutate the worktree directly — Claude's `Edit` writes to disk —
/// so they return `Envelope<()>` and the caller re-reads the file to see what
/// happened. That is why the pre-turn snapshot exists.
pub trait ClaudeDriver {
    /// Adjudicate one file's comment threads. Runs in a permission mode that
    /// structurally cannot modify files.
    fn verdict_turn(
        &self,
        session: &SessionId,
        prompt: &str,
    ) -> Result<Envelope<serde_json::Value>, DriverError>;

    /// Apply one accepted comment, editing inside `worktree`.
    fn apply_turn(
        &self,
        session: &SessionId,
        prompt: &str,
        worktree: &Path,
    ) -> Result<Envelope<()>, DriverError>;

    /// Draft a reply for one rejected thread.
    fn reply_turn(
        &self,
        session: &SessionId,
        prompt: &str,
    ) -> Result<Envelope<String>, DriverError>;
}

/// Failures driving the `claude` CLI.
#[derive(Debug, thiserror::Error)]
pub enum DriverError {
    #[error(
        "`claude` is not on PATH — Remendo drives the Claude Code CLI and cannot run without it"
    )]
    ClaudeNotFound,

    #[error("the {turn} turn reported an error")]
    TurnFailed { turn: String },

    #[error("the {turn} turn returned no structured output to read a payload from")]
    NoStructuredOutput { turn: String },

    #[error("could not run the {turn} turn: {message}")]
    Spawn { turn: String, message: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_successful_envelope_yields_its_payload() {
        let envelope = Envelope {
            is_error: false,
            total_cost_usd: 0.14,
            structured_output: Some(42),
        };
        assert_eq!(envelope.payload("verdict").unwrap(), 42);
    }

    #[test]
    fn a_failed_envelope_is_surfaced_rather_than_parsed() {
        let envelope: Envelope<i32> = Envelope {
            is_error: true,
            total_cost_usd: 0.02,
            structured_output: None,
        };
        let err = envelope.payload("verdict").unwrap_err();
        assert!(err.to_string().contains("verdict"), "names the turn");
    }

    /// A turn that reports success but returns nothing must not be treated as
    /// an empty result — a missing payload is not an absent one.
    #[test]
    fn a_missing_payload_is_an_error_not_an_empty_value() {
        let envelope: Envelope<Vec<i32>> = Envelope {
            is_error: false,
            total_cost_usd: 0.01,
            structured_output: None,
        };
        assert!(matches!(
            envelope.payload("verdict"),
            Err(DriverError::NoStructuredOutput { .. })
        ));
    }
}
