//! `remendo <change-id>` — the whole interface.
//!
//! Thin by design: this resolves the environment (which clone, which Gerrit,
//! which credential), opens the review workspace, and hands off. The rules it
//! enforces live in [`remendo::workspace`], not here.
//!
//! Output is plain text, per `config.yaml` — structured logging is for
//! debugging, never for what the user reads.

use std::process::ExitCode;

use remendo::app::{self, App, Outcome};
use remendo::apply::RealWorktree;
use remendo::gerrit::{
    GerritEvent, GerritHttp, GerritRequest, GerritWorker, LoadedChange, base_url,
};
use remendo::git::{GitCli, GitCommand};
use remendo::submit::{Fate, TriagedThread};
use remendo::theme::Theme;
use remendo::triage::Triage;
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
    let loaded = match worker.wait() {
        Some(GerritEvent::ChangeLoaded(loaded)) => loaded,
        Some(GerritEvent::Failed(err)) => return Err(err.to_string()),
        Some(GerritEvent::ReviewPosted) | None => {
            return Err("the Gerrit worker stopped without loading the change".into());
        }
    };
    report_threads(&loaded);
    if loaded.is_empty() {
        // Exiting gracefully rather than opening an empty UI (design.md §14).
        return Ok(());
    }

    triage(&workspace, *loaded)
}

/// Run the triage UI over a loaded change and report what came out of it.
///
/// The verdict pass is **not** wired: `claude-driver` is §4, gated on task 4.11
/// until the CLI is re-probed against 2.1.222. Every thread therefore arrives
/// unadjudicated, which the UI states explicitly rather than showing as an
/// empty verdict.
fn triage(workspace: &workspace::Workspace, loaded: LoadedChange) -> Result<(), String> {
    let worktree = workspace.worktree();
    let files = RealWorktree::new(&worktree);
    let mut app = App::new(
        Triage::new(
            loaded.threads.threads,
            &[],
            loaded.threads.skipped_older_patchsets,
        ),
        loaded.change,
        worktree.display().to_string(),
    );

    let outcome = app::run(
        &mut app,
        &files,
        &workspace.change.subject,
        &Theme::default(),
    )
    .map_err(|e| format!("terminal error: {e}"))?;

    match outcome {
        Outcome::Finished(fates) => {
            report_fates(&fates);
            // Apply (§6) and finalize (§7) are built and tested, but wiring
            // them here needs the apply turn, which is the same §4 gate.
            println!(
                "\nApplying and pushing needs the Claude driver (§4), which is gated on \
                 re-probing the CLI — see tasks.md 4.11. Nothing was pushed."
            );
        }
        Outcome::Aborted => println!("\n{}", workspace.abort_report()),
        Outcome::Running => unreachable!("run returns only once the app has stopped"),
    }
    Ok(())
}

/// Summarize what triage decided.
fn report_fates(fates: &[TriagedThread]) {
    let mut resolved = 0;
    let mut replied = 0;
    let mut untouched = 0;
    for item in fates {
        match &item.fate {
            Fate::Accepted | Fate::FixedByHand => resolved += 1,
            Fate::Rejected { reply: Some(_) } => replied += 1,
            Fate::Rejected { reply: None } | Fate::Skipped => untouched += 1,
        }
    }
    println!(
        "triage complete: {resolved} to resolve, {replied} to reply to, {untouched} left alone"
    );
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
        change.current_revision
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
