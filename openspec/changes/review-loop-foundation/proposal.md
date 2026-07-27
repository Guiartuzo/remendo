## Why

We use Gerrit + Git, and we adopted Claude Code — so we ship far more code and
the review volume exploded. In practice we almost never edit code by hand during
review anymore. The bottleneck is no longer *typing the fix*; it is the loop of
reading each inline comment, judging whether it is even correct, applying it, and
marking it resolved — one comment at a time, across many changes.

Remendo is a keyboard-driven terminal app that collapses that loop. Given a
Gerrit change id, it pulls the change into an isolated worktree, asks Claude Code
to **adjudicate** every unresolved reviewer comment (agree / disagree, with a
justification — because reviewers can be wrong), lets a human triage those
verdicts in a two-panel UI, applies the accepted ones as scoped edits shown as a
confirm-diff before anything is written, and finishes by amending one new
patchset, resolving the accepted comments, and posting drafted replies to the
comments we rejected.

Remendo is a **sibling to vybim**, not a mode inside it. An editor and a review
cockpit have opposite defaults (blank buffer + cursor vs. a change loaded with
comments queued), and we do not edit by hand anymore — so this is a new product
in its own repo. It borrows vybim's two hardest-won pieces by **copying and
diverging** (not a shared crate yet): the side-by-side diff renderer
(`diff_view.rs` + `similar`) and the subprocess-orchestration patterns
(`git.rs`'s spawn-wait-parse and `lsp/transport.rs`'s long-lived transport).

## What Changes

- **Launch by change id.** `remendo <change-id>` fetches the change, creates a
  dedicated git **worktree**, checks out the patchset into it, and loads the
  change's unresolved inline comments. The user's real working tree is never
  touched, so aborting mid-review is a non-event: the worktree is simply left in
  place (nothing pushed, nothing lost).
- **Gerrit REST over a worker thread — no async runtime.** All Gerrit access
  (fetch change + files, fetch unresolved comments, post the finalize review)
  runs on a background thread with a blocking HTTP client, talking to the UI over
  a channel — mirroring how vybim drives LSP. Responses have Gerrit's `)]}'`
  XSSI guard line stripped before JSON parsing.
- **Claude adjudicates, then applies — one session per change.** A single Claude
  Code session (resume-based one-shots keyed by a generated session id) is driven
  turn by turn: first a **verdict pass** in `plan` permission mode (structurally
  unable to edit), chunked one turn per file, returning schema-validated
  `agree/disagree/unsure` + justification + a required `depends_on` naming any
  out-of-code fact the verdict rests on; then, after triage, **per-comment apply**
  turns with tools scoped to `Read,Edit` (no shell). Edits compose because each
  turn reads the working tree the previous turn wrote.
- **Two-panel manual triage (rung 0 only).** Left: code + reviewer comment.
  Right: Claude's verdict, justification, and declared dependencies. The human
  accepts / rejects / edits each comment's prose, and approves a drafted reply for
  each rejection before leaving triage. No auto modes in v0.
- **Confirm before write.** Each accepted comment's edit is shown as a
  confirm-diff (the file's **pre-turn snapshot** vs. Claude's edit) reusing the
  ported diff renderer, so the diff shows only that comment's change. On confirm
  the edit is staged and stays in the worktree; rejecting restores the snapshot,
  preserving edits already confirmed for earlier comments in the same file.
  Nothing is pushed until the end.
- **Finalize once, git-native, unattended.** A pre-push check aborts if the author
  uploaded a new patchset during triage. Then `git commit --amend` folds all
  confirmed edits into a single new patchset and pushes it. Only **after** the
  push succeeds does Remendo issue one batched review post settling every comment
  fate — so Gerrit never shows a resolution or a "fixed" reply with no patchset
  behind it.

## Capabilities

### New Capabilities
- `review-workspace`: Launch-by-change-id, isolated git worktree lifecycle,
  patchset fetch/checkout, and abort-leaves-worktree behavior.
- `gerrit-client`: Blocking Gerrit REST access on a background worker thread —
  fetch change/files/comments, classify comment anchors (including the
  `/COMMIT_MSG` and `/PATCHSET_LEVEL` pseudo-paths), settle every comment fate in
  one batched review post, amend-and-push — with XSSI-prefix stripping and channel
  delivery to the UI.
- `claude-driver`: The shared Claude Code subprocess contract — one resume-based
  session per change, permission-mode and tool/dir scoping, structured-output
  verdicts — that both triage and apply build on.
- `comment-triage`: The verdict pass and the two-panel manual triage UI; the
  verdict × human-decision model.
- `fix-application`: Per-comment scoped apply via the resumed session, the
  snapshot-per-turn confirm-diff guard (reject restores the pre-turn snapshot),
  and worktree accumulation of confirmed edits.
- `change-submission`: The pre-push revision check, amend into one new patchset,
  push, then a single batched review post settling every comment fate — strictly
  after a successful push.

### Modified Capabilities
<!-- Greenfield project; no existing capability specs to modify. -->

## Impact

- **New repo**: `remendo` (github.com/Guiartuzo/remendo), Rust + ratatui,
  established as a sibling to vybim.
- **Reused-by-copy from vybim**: `diff_view.rs` (+ `similar`) as the confirm-diff
  surface; the `git.rs` spawn-wait-parse pattern for git and Claude one-shots;
  the `lsp/transport.rs` worker-thread + `AppEvent` channel pattern for Gerrit
  and (if later needed) a streaming Claude session. These are copied and will
  diverge; extracting a shared `vybim-core` crate is deferred until the
  duplication actually hurts.
- **Dependencies (anticipated)**: `crossterm` + `ratatui` (TUI), `similar`
  (diff), `serde` + `serde_json` (Gerrit JSON + Claude stream-json/JSON output),
  a **blocking** HTTP client (e.g. `ureq`, rustls) — deliberately **no `tokio`**;
  the synchronous render loop stays synchronous and the network lives on a worker
  thread.
- **External dependencies**: the `claude` CLI (Claude Code) on `PATH`, verified
  at v2.1.220 to support the driving model this design needs (`--session-id` /
  `--resume` — including resuming a `plan` verdict session in `acceptEdits` with
  context retained, tested end to end; `--output-format json` returning a result
  envelope with a `structured_output` object; `--json-schema` taking the schema
  inline; `--permission-mode plan`/`acceptEdits`, `--tools`, `--max-budget-usd`);
  and a Gerrit instance reachable over REST with credentials. This CLI surface is
  **not a stable API** — it is pinned to a verified version and re-probed on
  upgrade (design.md §10).

## Deferred (explicitly out of v0 scope)

- Auto modes (semi-auto rung 1, full-auto rung 2). v0 is manual (rung 0) only.
- Multi-file / cross-file refactors; conflict resolution between edits.
- Comment threading beyond top-level unresolved comments.
- Multi-change dashboards.
- A live bidirectional streaming Claude session (architecture A). v0 uses
  resume-based one-shots (architecture B); streaming is a later optimization.
- Extracting a shared `vybim-core` crate.
- Final **prose tuning** of the three prompts. Their roles, scoping, I/O
  contracts, and granularity (per comment for apply, per file for verdicts) are
  settled (see design.md §7 "Prompt contract"); only exact wording and how much
  surrounding code each turn is shown remain (design.md §11).
- Batching a comment-dense file's comments into one apply turn — a localized cost
  optimization; v0 is uniformly one turn per comment.
