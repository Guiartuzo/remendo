//! Reading and writing worktree files, behind a project-owned trait.
//!
//! `config.yaml` requires external I/O — the filesystem included — to be mocked
//! with a named fake. That is not ceremony here: the snapshot-and-revert rule
//! this module exists to protect is a *sequence* of reads and writes, and an
//! in-memory fake lets those be asserted exactly rather than inferred from
//! whatever a temp directory happens to contain.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use super::ApplyError;

/// Reading and writing files inside the review worktree.
///
/// Paths are worktree-relative, which keeps a caller from reaching outside it
/// by accident and keeps `CommentAnchor`'s pseudo-paths unusable here.
pub trait WorktreeFiles {
    fn read(&self, path: &str) -> Result<String, ApplyError>;
    fn write(&self, path: &str, contents: &str) -> Result<(), ApplyError>;
}

/// The real worktree on disk.
#[derive(Debug, Clone)]
pub struct RealWorktree {
    root: PathBuf,
}

impl RealWorktree {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }
}

impl WorktreeFiles for RealWorktree {
    fn read(&self, path: &str) -> Result<String, ApplyError> {
        let full = self.root.join(path);
        std::fs::read_to_string(&full).map_err(|source| ApplyError::Io { path: full, source })
    }

    fn write(&self, path: &str, contents: &str) -> Result<(), ApplyError> {
        let full = self.root.join(path);
        std::fs::write(&full, contents).map_err(|source| ApplyError::Io { path: full, source })
    }
}

/// An in-memory worktree.
///
/// Cloning shares the same files, so an apply loop and the [`FakeDriver`] that
/// edits behind its back can hold the same worktree — which is exactly the
/// relationship the real ones have.
///
/// [`FakeDriver`]: crate::driver::FakeDriver
#[derive(Debug, Clone, Default)]
pub struct FakeWorktree {
    files: Rc<RefCell<HashMap<String, String>>>,
}

impl FakeWorktree {
    /// A worktree pre-populated with `(path, contents)` pairs.
    pub fn with_files(files: &[(&str, &str)]) -> Self {
        let worktree = Self::default();
        for (path, contents) in files {
            worktree.set(path, contents);
        }
        worktree
    }

    /// Put a file in place without going through the trait.
    pub fn set(&self, path: &str, contents: &str) {
        self.files
            .borrow_mut()
            .insert(path.to_string(), contents.to_string());
    }

    /// A file's contents, for assertions.
    pub fn contents(&self, path: &str) -> Option<String> {
        self.files.borrow().get(path).cloned()
    }
}

impl WorktreeFiles for FakeWorktree {
    fn read(&self, path: &str) -> Result<String, ApplyError> {
        self.files
            .borrow()
            .get(path)
            .cloned()
            .ok_or_else(|| ApplyError::Io {
                path: Path::new(path).to_path_buf(),
                source: std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    "no such file in the fake worktree",
                ),
            })
    }

    fn write(&self, path: &str, contents: &str) -> Result<(), ApplyError> {
        self.set(path, contents);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fake_worktree_round_trips_a_file() {
        let wt = FakeWorktree::with_files(&[("a.rs", "fn a() {}\n")]);
        assert_eq!(wt.read("a.rs").unwrap(), "fn a() {}\n");
        wt.write("a.rs", "fn a() { one(); }\n").unwrap();
        assert_eq!(wt.read("a.rs").unwrap(), "fn a() { one(); }\n");
    }

    #[test]
    fn a_missing_file_names_itself() {
        let err = FakeWorktree::default().read("nope.rs").unwrap_err();
        assert!(err.to_string().contains("nope.rs"), "{err}");
    }

    /// Clones must share, or a driver holding one could not be seen editing by
    /// the loop holding the other.
    #[test]
    fn clones_share_the_same_files() {
        let wt = FakeWorktree::with_files(&[("a.rs", "before")]);
        let other = wt.clone();
        other.write("a.rs", "after").unwrap();
        assert_eq!(wt.read("a.rs").unwrap(), "after");
    }

    #[test]
    fn a_real_worktree_reads_and_writes_under_its_root() {
        let root = std::env::temp_dir().join("remendo-files-test");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();

        let wt = RealWorktree::new(&root);
        wt.write("a.rs", "hello\n").unwrap();
        assert_eq!(wt.read("a.rs").unwrap(), "hello\n");
        assert_eq!(
            std::fs::read_to_string(root.join("a.rs")).unwrap(),
            "hello\n"
        );

        let _ = std::fs::remove_dir_all(&root);
    }
}
