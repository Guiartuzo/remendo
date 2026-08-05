//! Remendo — a keyboard-driven terminal cockpit for adjudicating and applying
//! Gerrit review comments.
//!
//! The crate is split lib + bin so the modules are addressable from doctests
//! and integration tests; `main.rs` stays a thin entrypoint over it.
//!
//! Several modules here are **copied from vybim and diverged** rather than
//! shared through a crate (proposal.md): `theme`, `syntax`, `buffer`,
//! `file_tree` and `minibuffer`. Where a copy has drifted from its origin the
//! module says so at its divergence point, so the two can be diffed later if a
//! `vybim-core` crate is ever extracted.

pub mod apply;
pub mod buffer;
pub mod diff_view;
pub mod driver;
pub mod file_tree;
pub mod gerrit;
pub mod git;
pub mod minibuffer;
pub mod pane;
pub mod search;
pub mod submit;
pub mod syntax;
pub mod theme;
pub mod triage;
pub mod verdict;
pub mod workspace;
