//! `remendo <change-id>` — the whole interface.
//!
//! Thin by design: this resolves the environment (which clone, which Gerrit,
//! which credential), opens the review workspace, and hands off. The rules it
//! enforces live in [`remendo::workspace`], not here.
//!
//! Output is plain text, per `config.yaml` — structured logging is for
//! debugging, never for what the user reads.

use std::process::ExitCode;

use remendo::gerrit::{GerritEvent, GerritHttp, GerritRequest, GerritWorker, base_url};
use remendo::git::{GitCli, GitCommand};
use remendo::workspace::{self, paths};

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("remendo: {message}");
            ExitCode::FAILURE
        }
    }
}

/// The launch sequence. Every failure is a plain message; nothing panics at the
/// user.
fn run() -> Result<(), String> {
    let Some(change_id) = std::env::args().nth(1) else {
        eprintln!("usage: remendo <change-id>");
        eprintln!("\nRun from inside a clone of the change's project.");
        return Err("no change id given".into());
    };
    if change_id == "--version" || change_id == "-V" {
        println!("remendo {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }

    let git = GitCommand::new();
    // The cwd-in-clone check comes FIRST, before anything that depends on the
    // clone. `connect` reads the origin remote, and outside a clone that fails
    // as "remote `origin` has no URL" — which blames the wrong thing and sends
    // the user looking at their remotes instead of their directory.
    git.repo_root().map_err(|e| e.to_string())?;

    let api = connect(&git)?;
    let state_dir = paths::state_dir().map_err(|e| e.to_string())?;

    let workspace =
        workspace::open(&git, &api, &change_id, &state_dir).map_err(|e| e.to_string())?;
    report_workspace(&workspace);

    // The comment fetch goes over the worker thread even though there is no UI
    // to keep responsive yet, so the seam is exercised from the start rather
    // than retrofitted once the TUI lands (§5). `api` moves onto the thread.
    let worker = GerritWorker::spawn(api);
    if !worker.request(GerritRequest::LoadChange {
        change_id: change_id.clone(),
    }) {
        return Err("the Gerrit worker stopped before it could be asked".into());
    }
    match worker.wait() {
        Some(GerritEvent::ChangeLoaded(loaded)) => report_threads(&loaded),
        Some(GerritEvent::Failed(err)) => return Err(err.to_string()),
        Some(GerritEvent::ReviewPosted) | None => {
            return Err("the Gerrit worker stopped without loading the change".into());
        }
    }
    Ok(())
}

/// Build a Gerrit client from the clone: origin remote → base URL → credential.
fn connect(git: &GitCommand) -> Result<GerritHttp, String> {
    let origin = git.remote_url("origin").map_err(|e| e.to_string())?;
    let base = base_url::derive(&origin)
        .ok_or_else(|| format!("could not derive Gerrit's URL from origin `{origin}`"))?;
    let host = base_url::host_of(&base)
        .ok_or_else(|| format!("could not read a host from the derived URL `{base}`"))?;

    let credential = git.fill_credential(host).map_err(|e| e.to_string())?;
    // Git's own CA setting is where a corporate root already lives for anyone
    // whose `git push` works; it is only used to make a TLS failure actionable.
    let ca_info = git.config_get("http.sslCAInfo").ok().flatten();
    Ok(GerritHttp::new(&base, &credential, ca_info))
}

fn report_workspace(workspace: &workspace::Workspace) {
    let change = &workspace.change;
    println!("{} — {}", change.id, change.subject);
    println!("  project  {}", change.project);
    println!("  branch   {}", change.branch);
    println!(
        "  patchset {} ({})",
        change.current_patch_set(),
        &change.current_revision
    );
    let how = if workspace.resumed {
        "resumed"
    } else {
        "created"
    };
    println!("  worktree {} ({how})", workspace.worktree().display());
}

fn report_threads(loaded: &remendo::gerrit::LoadedChange) {
    let set = &loaded.threads;
    if loaded.is_empty() {
        println!("\nNo unresolved comment threads on the current patchset.");
    } else {
        println!("\n{} unresolved thread(s) to triage:", set.threads.len());
        for thread in &set.threads {
            let where_ = match thread.anchor.file_path() {
                Some(path) => match thread.line() {
                    Some(line) => format!("{path}:{line}"),
                    None => path.to_string(),
                },
                None => format!("{:?}", thread.anchor),
            };
            println!(
                "  {where_}  ({} comment(s), by {})",
                thread.comments.len(),
                thread.root().author_name()
            );
        }
    }
    // A skipped thing must never look like an absent thing (design.md §13).
    if set.skipped_older_patchsets > 0 {
        println!(
            "\n{} unresolved thread(s) on earlier patchsets were NOT triaged — \
             their line anchors address code that has since changed.",
            set.skipped_older_patchsets
        );
    }
}
