//! [`GitCli`] over the real `git` binary, using the spawn-wait-parse shape
//! copied from vybim's `git.rs`.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use super::{Credential, GitCli, GitError};

/// Drives the `git` executable found on `PATH`.
#[derive(Debug, Clone, Default)]
pub struct GitCommand {
    /// Directory commands run in. `None` uses the process's cwd.
    cwd: Option<PathBuf>,
}

impl GitCommand {
    /// A driver running git in the process's current directory.
    pub fn new() -> Self {
        Self::default()
    }

    /// A driver running git inside `dir`.
    pub fn in_dir(dir: impl Into<PathBuf>) -> Self {
        Self {
            cwd: Some(dir.into()),
        }
    }

    /// Run git with `args`, returning its raw output.
    fn output(&self, args: &[&str]) -> Result<Output, GitError> {
        let mut command = Command::new("git");
        command.args(args);
        if let Some(dir) = &self.cwd {
            command.current_dir(dir);
        }
        command.output().map_err(|source| {
            if source.kind() == std::io::ErrorKind::NotFound {
                GitError::GitNotFound
            } else {
                GitError::Spawn {
                    command: args.join(" "),
                    source,
                }
            }
        })
    }

    /// Run git and require success, returning trimmed stdout.
    fn run(&self, args: &[&str]) -> Result<String, GitError> {
        let out = self.output(args)?;
        if !out.status.success() {
            return Err(GitError::CommandFailed {
                command: args.join(" "),
                status: out.status.to_string(),
                stderr: String::from_utf8_lossy(&out.stderr).trim().to_string(),
            });
        }
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    }
}

impl GitCli for GitCommand {
    fn repo_root(&self) -> Result<PathBuf, GitError> {
        let out = self.output(&["rev-parse", "--show-toplevel"])?;
        if !out.status.success() {
            let cwd = self
                .cwd
                .clone()
                .or_else(|| std::env::current_dir().ok())
                .unwrap_or_default();
            return Err(GitError::NotAClone { cwd });
        }
        let root = String::from_utf8_lossy(&out.stdout).trim().to_string();
        Ok(PathBuf::from(root))
    }

    fn remote_url(&self, remote: &str) -> Result<String, GitError> {
        let out = self.output(&["remote", "get-url", remote])?;
        if !out.status.success() {
            return Err(GitError::NoSuchRemote {
                remote: remote.to_string(),
            });
        }
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    }

    fn config_get(&self, key: &str) -> Result<Option<String>, GitError> {
        let out = self.output(&["config", "--get", key])?;
        // A non-zero exit here means the key is unset, which is not a failure.
        if !out.status.success() {
            return Ok(None);
        }
        let value = String::from_utf8_lossy(&out.stdout).trim().to_string();
        Ok((!value.is_empty()).then_some(value))
    }

    fn fill_credential(&self, host: &str) -> Result<Credential, GitError> {
        let request = format!("protocol=https\nhost={host}\n\n");
        let mut command = Command::new("git");
        command
            .args(["credential", "fill"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(dir) = &self.cwd {
            command.current_dir(dir);
        }

        let mut child = command.spawn().map_err(|source| {
            if source.kind() == std::io::ErrorKind::NotFound {
                GitError::GitNotFound
            } else {
                GitError::Spawn {
                    command: "credential fill".into(),
                    source,
                }
            }
        })?;
        child
            .stdin
            .take()
            .expect("stdin was piped")
            .write_all(request.as_bytes())
            .map_err(|source| GitError::Spawn {
                command: "credential fill".into(),
                source,
            })?;

        let out = child.wait_with_output().map_err(|source| GitError::Spawn {
            command: "credential fill".into(),
            source,
        })?;
        if !out.status.success() {
            return Err(GitError::NoCredential {
                host: host.to_string(),
            });
        }
        parse_credential(&String::from_utf8_lossy(&out.stdout), host)
    }

    fn fetch(&self, remote: &str, refspec: &str) -> Result<(), GitError> {
        self.run(&["fetch", remote, refspec]).map(drop)
    }

    fn worktree_add(&self, path: &Path, revision: &str) -> Result<(), GitError> {
        let path = path.to_string_lossy();
        self.run(&["worktree", "add", "--detach", &path, revision])
            .map(drop)
    }

    fn stage(&self, worktree: &Path, path: &str) -> Result<(), GitError> {
        GitCommand::in_dir(worktree)
            .run(&["add", "--", path])
            .map(drop)
    }

    fn commit_amend(&self, worktree: &Path, message: Option<&str>) -> Result<(), GitError> {
        let git = GitCommand::in_dir(worktree);
        match message {
            Some(message) => git.run(&["commit", "--amend", "-m", message]).map(drop),
            None => git.run(&["commit", "--amend", "--no-edit"]).map(drop),
        }
    }

    fn push(&self, worktree: &Path, refspec: &str) -> Result<(), GitError> {
        GitCommand::in_dir(worktree)
            .run(&["push", "origin", refspec])
            .map(drop)
    }
}

/// Parse `git credential fill`'s `key=value` output.
///
/// The helper echoes back what it was given plus whatever it resolved, so the
/// response carries `protocol` and `host` alongside `username`/`password`. A
/// helper that ran but produced no password is treated as no credential, since
/// issuing an unauthenticated request would fail confusingly later.
fn parse_credential(stdout: &str, host: &str) -> Result<Credential, GitError> {
    let mut username = None;
    let mut password = None;
    for line in stdout.lines() {
        match line.split_once('=') {
            Some(("username", value)) => username = Some(value.to_string()),
            Some(("password", value)) => password = Some(value.to_string()),
            _ => {}
        }
    }
    match (username, password) {
        (Some(username), Some(password)) => Ok(Credential { username, password }),
        _ => Err(GitError::NoCredential {
            host: host.to_string(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credential_output_is_parsed() {
        let stdout = "protocol=https\nhost=gerrit.corp\nusername=guilherme\npassword=s3cret\n";
        let cred = parse_credential(stdout, "gerrit.corp").unwrap();
        assert_eq!(cred.username, "guilherme");
        assert_eq!(cred.password, "s3cret");
    }

    #[test]
    fn a_password_containing_equals_survives_parsing() {
        // Only the FIRST `=` separates key from value; tokens are common in
        // passwords and must not be truncated.
        let cred = parse_credential("username=u\npassword=ab==cd=\n", "h").unwrap();
        assert_eq!(cred.password, "ab==cd=");
    }

    #[test]
    fn a_helper_returning_no_password_is_no_credential() {
        let err = parse_credential("username=u\n", "gerrit.corp").unwrap_err();
        assert!(matches!(err, GitError::NoCredential { .. }));
        assert!(err.to_string().contains("gerrit.corp"), "names the host");
    }

    #[test]
    fn unrecognized_lines_are_ignored() {
        let stdout = "capability[]=authtype\nusername=u\npassword=p\nquit=0\n";
        assert!(parse_credential(stdout, "h").is_ok());
    }

    /// A credential must never reach a log line or a panic message.
    #[test]
    fn debug_redacts_the_password() {
        let cred = Credential {
            username: "guilherme".into(),
            password: "s3cret".into(),
        };
        let shown = format!("{cred:?}");
        assert!(shown.contains("guilherme"));
        assert!(!shown.contains("s3cret"), "password leaked: {shown}");
        assert!(shown.contains("redacted"));
    }

    /// These run against the real `git` in this repository, which is a clone.
    #[test]
    fn repo_root_finds_this_clone() {
        let root = GitCommand::new().repo_root().expect("tests run in a clone");
        assert!(root.join(".git").exists());
    }

    #[test]
    fn an_unset_config_key_is_none_not_an_error() {
        let value = GitCommand::new()
            .config_get("remendo.definitely.unset.key")
            .expect("an unset key is not a failure");
        assert_eq!(value, None);
    }

    #[test]
    fn a_missing_remote_names_itself() {
        let err = GitCommand::new().remote_url("nope").unwrap_err();
        assert!(err.to_string().contains("nope"), "{err}");
    }
}
