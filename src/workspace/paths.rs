//! Where a review's session lives on disk.
//!
//! Under the user's state directory rather than inside the clone, so a session
//! survives `git clean` and does not pollute the repository (design.md §13).
//!
//! ```text
//!   $XDG_STATE_HOME/remendo/<project>/<change-id>/
//!                                     ├── worktree/      the checked-out patchset
//!                                     └── verdicts.json  the (change,revision) cache
//! ```
//!
//! REFINEMENT of `tasks.md` 3.2, which named the change directory itself as the
//! worktree. The cache has to sit *beside* the worktree rather than inside it —
//! anything inside would land in the checkout and show up as an untracked file
//! in the change under review — so the change directory became a session
//! directory holding both.

use std::path::{Path, PathBuf};

use super::WorkspaceError;

/// Directory name for the checkout inside a session directory.
const WORKTREE_DIR: &str = "worktree";

/// File name for the verdict cache inside a session directory.
const VERDICTS_FILE: &str = "verdicts.json";

/// The paths making up one change's review session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionPaths {
    /// The session directory: `<state>/remendo/<project>/<change-id>/`.
    pub root: PathBuf,
}

impl SessionPaths {
    /// Build the session paths for a change, rooted at `state_dir`.
    ///
    /// ```
    /// # use std::path::Path;
    /// # use remendo::workspace::SessionPaths;
    /// let paths = SessionPaths::new(Path::new("/state"), "platform/base", "12345").unwrap();
    /// assert!(paths.worktree().ends_with("worktree"));
    /// ```
    pub fn new(state_dir: &Path, project: &str, change_id: &str) -> Result<Self, WorkspaceError> {
        let project = safe_component(project, "project")?;
        let change_id = safe_component(change_id, "change id")?;
        Ok(Self {
            root: state_dir.join("remendo").join(project).join(change_id),
        })
    }

    /// Where the patchset is checked out.
    pub fn worktree(&self) -> PathBuf {
        self.root.join(WORKTREE_DIR)
    }

    /// Where the verdict cache is written.
    pub fn verdicts(&self) -> PathBuf {
        self.root.join(VERDICTS_FILE)
    }

    /// Whether a worktree already exists here — which makes a launch a resume
    /// rather than a fresh checkout.
    pub fn worktree_exists(&self) -> bool {
        self.worktree().is_dir()
    }
}

/// The user's state directory: `$XDG_STATE_HOME`, else `$HOME/.local/state`.
pub fn state_dir() -> Result<PathBuf, WorkspaceError> {
    if let Some(dir) = std::env::var_os("XDG_STATE_HOME").filter(|d| !d.is_empty()) {
        return Ok(PathBuf::from(dir));
    }
    let home = std::env::var_os("HOME")
        .filter(|h| !h.is_empty())
        .ok_or(WorkspaceError::NoStateDir)?;
    Ok(PathBuf::from(home).join(".local").join("state"))
}

/// Validate a path component that came from outside.
///
/// A Gerrit project legitimately contains `/` and becomes nested directories,
/// which is readable and collision-free. What it must never contain is a `..`
/// component or a leading `/`: both would place the session outside the state
/// directory, and the project name arrives from a REST response.
fn safe_component(value: &str, what: &str) -> Result<String, WorkspaceError> {
    let trimmed = value.trim_matches('/');
    let rejected = trimmed.is_empty()
        || trimmed
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == ".." || part.contains('\\'));
    if rejected {
        return Err(WorkspaceError::UnsafeName {
            what: what.to_string(),
            value: value.to_string(),
        });
    }
    Ok(trimmed.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths(project: &str, change: &str) -> Result<SessionPaths, WorkspaceError> {
        SessionPaths::new(Path::new("/state"), project, change)
    }

    #[test]
    fn a_session_nests_project_then_change() {
        let p = paths("platform/base", "12345").unwrap();
        assert_eq!(p.root, PathBuf::from("/state/remendo/platform/base/12345"));
        assert_eq!(
            p.worktree(),
            PathBuf::from("/state/remendo/platform/base/12345/worktree")
        );
        assert_eq!(
            p.verdicts(),
            PathBuf::from("/state/remendo/platform/base/12345/verdicts.json")
        );
    }

    /// The cache must not live inside the checkout, or it would appear as an
    /// untracked file in the change being reviewed.
    #[test]
    fn the_cache_sits_beside_the_worktree_not_inside_it() {
        let p = paths("proj", "1").unwrap();
        assert!(!p.verdicts().starts_with(p.worktree()));
        assert_eq!(p.verdicts().parent(), Some(p.root.as_path()));
    }

    #[test]
    fn different_changes_get_different_sessions() {
        assert_ne!(paths("proj", "1").unwrap(), paths("proj", "2").unwrap());
        assert_ne!(paths("a", "1").unwrap(), paths("b", "1").unwrap());
    }

    /// The project name arrives from a REST response, so traversal has to be
    /// impossible rather than merely unlikely.
    #[test]
    fn traversal_in_a_project_name_is_refused() {
        for bad in ["../../etc", "a/../../b", "..", "a/./b", "a//b"] {
            let err = paths(bad, "1").unwrap_err();
            assert!(
                matches!(err, WorkspaceError::UnsafeName { .. }),
                "{bad} was accepted"
            );
            assert!(err.to_string().contains(bad), "error names the value");
        }
    }

    #[test]
    fn traversal_in_a_change_id_is_refused() {
        assert!(paths("proj", "../../etc").is_err());
        assert!(paths("proj", "").is_err());
    }

    #[test]
    fn surrounding_slashes_are_tolerated() {
        assert_eq!(
            paths("/proj/", "1").unwrap().root,
            PathBuf::from("/state/remendo/proj/1")
        );
    }

    #[test]
    fn xdg_state_home_wins_when_set() {
        // Guarded rather than parallel-safe: env is process-global, so this
        // test sets and restores it around one assertion.
        let previous = std::env::var_os("XDG_STATE_HOME");
        unsafe { std::env::set_var("XDG_STATE_HOME", "/custom/state") };
        assert_eq!(state_dir().unwrap(), PathBuf::from("/custom/state"));
        match previous {
            Some(value) => unsafe { std::env::set_var("XDG_STATE_HOME", value) },
            None => unsafe { std::env::remove_var("XDG_STATE_HOME") },
        }
    }
}
