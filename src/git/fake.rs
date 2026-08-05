//! A named fake [`GitCli`] for tests.
//!
//! `config.yaml` requires external I/O to be mocked with a named fake type
//! implementing the project's own trait, not inline closures or ad-hoc stubs.
//! This one returns canned values and records the mutating calls it received,
//! so a test can assert *what git was asked to do* without running git.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use super::{Credential, GitCli, GitError};

/// A mutating git call, recorded in order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GitCall {
    Fetch {
        remote: String,
        refspec: String,
    },
    WorktreeAdd {
        path: PathBuf,
        revision: String,
    },
    Stage {
        worktree: PathBuf,
        path: String,
    },
    CommitAmend {
        worktree: PathBuf,
        message: Option<String>,
    },
    Push {
        worktree: PathBuf,
        refspec: String,
    },
}

/// A [`GitCli`] that answers from canned data and records what it was asked.
#[derive(Debug, Default)]
pub struct FakeGit {
    pub repo_root: Option<PathBuf>,
    pub remotes: HashMap<String, String>,
    pub config: HashMap<String, String>,
    pub credential: Option<Credential>,
    /// Mutating calls, in the order they arrived.
    calls: RefCell<Vec<GitCall>>,
}

impl FakeGit {
    /// A fake standing in a clone at `root`, with an `origin` pointing at
    /// `origin_url` and a credential that always resolves.
    pub fn in_clone(root: impl Into<PathBuf>, origin_url: &str) -> Self {
        let mut remotes = HashMap::new();
        remotes.insert("origin".to_string(), origin_url.to_string());
        Self {
            repo_root: Some(root.into()),
            remotes,
            config: HashMap::new(),
            credential: Some(Credential {
                username: "tester".into(),
                password: "token".into(),
            }),
            calls: RefCell::new(Vec::new()),
        }
    }

    /// A fake that is not inside a clone.
    pub fn outside_a_clone() -> Self {
        Self::default()
    }

    /// Set a config key the fake will report.
    pub fn with_config(mut self, key: &str, value: &str) -> Self {
        self.config.insert(key.to_string(), value.to_string());
        self
    }

    /// Make credential lookup fail, as an unconfigured helper would.
    pub fn without_credential(mut self) -> Self {
        self.credential = None;
        self
    }

    /// The mutating calls this fake received, in order.
    pub fn calls(&self) -> Vec<GitCall> {
        self.calls.borrow().clone()
    }

    fn record(&self, call: GitCall) {
        self.calls.borrow_mut().push(call);
    }
}

impl GitCli for FakeGit {
    fn repo_root(&self) -> Result<PathBuf, GitError> {
        self.repo_root.clone().ok_or_else(|| GitError::NotAClone {
            cwd: PathBuf::from("/tmp/not-a-clone"),
        })
    }

    fn remote_url(&self, remote: &str) -> Result<String, GitError> {
        self.remotes
            .get(remote)
            .cloned()
            .ok_or_else(|| GitError::NoSuchRemote {
                remote: remote.to_string(),
            })
    }

    fn config_get(&self, key: &str) -> Result<Option<String>, GitError> {
        Ok(self.config.get(key).cloned())
    }

    fn fill_credential(&self, host: &str) -> Result<Credential, GitError> {
        self.credential
            .clone()
            .ok_or_else(|| GitError::NoCredential {
                host: host.to_string(),
            })
    }

    fn fetch(&self, remote: &str, refspec: &str) -> Result<(), GitError> {
        self.record(GitCall::Fetch {
            remote: remote.to_string(),
            refspec: refspec.to_string(),
        });
        Ok(())
    }

    fn worktree_add(&self, path: &Path, revision: &str) -> Result<(), GitError> {
        self.record(GitCall::WorktreeAdd {
            path: path.to_path_buf(),
            revision: revision.to_string(),
        });
        Ok(())
    }

    fn stage(&self, worktree: &Path, path: &str) -> Result<(), GitError> {
        self.record(GitCall::Stage {
            worktree: worktree.to_path_buf(),
            path: path.to_string(),
        });
        Ok(())
    }

    fn commit_amend(&self, worktree: &Path, message: Option<&str>) -> Result<(), GitError> {
        self.record(GitCall::CommitAmend {
            worktree: worktree.to_path_buf(),
            message: message.map(str::to_string),
        });
        Ok(())
    }

    fn push(&self, worktree: &Path, refspec: &str) -> Result<(), GitError> {
        self.record(GitCall::Push {
            worktree: worktree.to_path_buf(),
            refspec: refspec.to_string(),
        });
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_clone_fake_answers_root_and_origin() {
        let git = FakeGit::in_clone("/repo", "https://gerrit.corp/a/proj");
        assert_eq!(git.repo_root().unwrap(), PathBuf::from("/repo"));
        assert_eq!(
            git.remote_url("origin").unwrap(),
            "https://gerrit.corp/a/proj"
        );
    }

    #[test]
    fn outside_a_clone_repo_root_fails() {
        let err = FakeGit::outside_a_clone().repo_root().unwrap_err();
        assert!(matches!(err, GitError::NotAClone { .. }));
    }

    #[test]
    fn an_unset_config_key_is_none() {
        let git = FakeGit::in_clone("/repo", "u").with_config("http.sslCAInfo", "/ca.pem");
        assert_eq!(
            git.config_get("http.sslCAInfo").unwrap().as_deref(),
            Some("/ca.pem")
        );
        assert_eq!(git.config_get("http.other").unwrap(), None);
    }

    #[test]
    fn a_fake_without_a_credential_names_the_host() {
        let git = FakeGit::in_clone("/repo", "u").without_credential();
        let err = git.fill_credential("gerrit.corp").unwrap_err();
        assert!(err.to_string().contains("gerrit.corp"));
    }

    #[test]
    fn mutating_calls_are_recorded_in_order() {
        let git = FakeGit::in_clone("/repo", "u");
        git.fetch("origin", "refs/changes/45/12345/3").unwrap();
        git.stage(Path::new("/wt"), "src/a.rs").unwrap();
        git.push(Path::new("/wt"), "HEAD:refs/for/main").unwrap();
        assert_eq!(
            git.calls(),
            vec![
                GitCall::Fetch {
                    remote: "origin".into(),
                    refspec: "refs/changes/45/12345/3".into()
                },
                GitCall::Stage {
                    worktree: "/wt".into(),
                    path: "src/a.rs".into()
                },
                GitCall::Push {
                    worktree: "/wt".into(),
                    refspec: "HEAD:refs/for/main".into()
                },
            ]
        );
    }

    /// Finalize must rewrite the message when a /COMMIT_MSG comment was
    /// accepted, so the fake has to distinguish the two amend shapes.
    #[test]
    fn amend_records_whether_the_message_was_rewritten() {
        let git = FakeGit::in_clone("/repo", "u");
        git.commit_amend(Path::new("/wt"), None).unwrap();
        git.commit_amend(Path::new("/wt"), Some("new subject"))
            .unwrap();
        assert_eq!(
            git.calls(),
            vec![
                GitCall::CommitAmend {
                    worktree: "/wt".into(),
                    message: None
                },
                GitCall::CommitAmend {
                    worktree: "/wt".into(),
                    message: Some("new subject".into())
                },
            ]
        );
    }
}
