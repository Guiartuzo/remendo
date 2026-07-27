> ⚠ **Read `open-decisions.md` first.** Several tasks below are blocked on
> decisions that have not been made — notably 2.1/2.3 (Gerrit configuration and
> auth), 3.1/3.2 (which clone, where the worktree lives, what a relaunch does),
> 2.4/2.6 (Gerrit thread semantics and comment provenance) and 4.3 (the verdict
> schema's concrete shape, including whether `depends_on.verify` is executed).
> Starting those before the decision lands means guessing.

## 1. Project scaffold

- [ ] 1.1 Initialize the Rust crate (`remendo`), edition 2024; set up CI (fmt +
      clippy + test) mirroring vybim's gates.
- [ ] 1.2 Add dependencies: `crossterm`, `ratatui`, `similar`, `ropey`, `ignore`,
      `tree-sitter` + `tree-sitter-highlight` + grammars, `serde` (derive),
      `serde_json`, and a blocking HTTP client (`ureq` + rustls). Confirm **no**
      `tokio`, and **no** `portable-pty`/`vt100` — v0 embeds no terminal.
- [ ] 1.3 Copy `diff_view.rs` (+ its `Theme`/`git` seams) from vybim and adapt it
      to take an arbitrary before/after text pair (pre-turn snapshot vs edit), not
      just HEAD-vs-working-tree.
- [ ] 1.4 Copy `theme.rs`, `syntax.rs`, and `buffer.rs` from vybim. `buffer.rs`
      ports whole (448 lines, no crate-internal deps); its mutation methods go
      unused in v0 but its file loading and line indexing are what the read-only
      viewer needs.
- [ ] 1.5 Port the **read-only** slice of `pane.rs`: syntax-highlighted viewport
      render, scrolling (+ its existing viewport tests), `goto_line`, and search.
      Leave behind multi-caret, insert/delete/undo, word motion and completion —
      roughly two thirds of the module (design.md §11).
- [ ] 1.6 Copy `file_tree.rs` (357 lines, depends only on `theme`) and
      `minibuffer.rs` (comment-prose input).

## 2. Gerrit client (`gerrit-client`)

- [ ] 2.1 Worker thread owning a blocking HTTP client; a `GerritEvent` channel to
      the UI. No async runtime.
- [ ] 2.2 Response helper that strips the `)]}'` XSSI prefix before `serde_json`.
- [ ] 2.3 Decide and wire the auth mechanism (cookie / HTTP password / .netrc).
- [ ] 2.4 Fetch: change + `current_revision` + `branch` + files; inline comments →
      filter unresolved → map to (file, line/range, author, message, comment id).
      Retain `current_revision` and `branch` — finalize needs both.
- [ ] 2.5 Classify comment anchors into a `CommentAnchor` enum: real file path,
      `/COMMIT_MSG` (commit message; resolve the line against the real message,
      accounting for Gerrit's synthetic Parent/Author/Commit header offset), or
      `/PATCHSET_LEVEL` (change-level, no path). Never derive an on-disk path or
      directory from the two pseudo-paths.
- [ ] 2.6 Mutation: **one** batched review POST to
      `/changes/{id}/revisions/current/review` carrying every comment fate —
      accepted with `unresolved: false`, rejected-with-reply with
      `unresolved: true`. There is no mark-resolved endpoint; do not model one.
      Used only in finalize.
- [ ] 2.7 Unit-test XSSI stripping, anchor classification (including the
      `/COMMIT_MSG` line offset), and comment mapping against captured fixtures.

## 3. Workspace + patchset checkout (`review-workspace`)

- [ ] 3.1 `remendo <change-id>` entrypoint; error-and-exit on unknown/inaccessible
      change **before** any worktree is created.
- [ ] 3.2 Create a dedicated git worktree; `git fetch` the revision ref and check
      it out into it. Never touch the user's real working tree.
- [ ] 3.3 Abort path: leave the worktree + confirmed edits in place, push nothing,
      report the worktree location.

## 4. Claude driver (`claude-driver`)

- [ ] 4.1 Generate a session UUID per change; spawn-wait-parse wrapper around
      `claude -p` (the `git.rs` shape), behind the project's own driver trait.
- [ ] 4.2 Envelope decoding: `--output-format json` returns a **result envelope**,
      not the payload. Deserialize it, gate on `is_error`, and read the payload
      from `structured_output` (a native object) — not from `result` (the same
      JSON as a string). Do NOT gate on `stop_reason == "end_turn"`; the
      structured-output path returns `tool_use`. Cover with a named fake
      implementing the driver trait, per `config.yaml`'s mock-external-IO rule.
- [ ] 4.3 Verdict schema + turn: `--permission-mode plan --output-format json
      --json-schema <schema JSON inline — a file path is rejected>`. Schema
      requires `{verdict, justification, confidence, depends_on}` with
      `depends_on` **required and nullable, never omittable**. Issue **one turn
      per file**, not one over the whole change.
- [ ] 4.4 Apply turn: `--resume <uuid> --tools "Read,Edit" --permission-mode
      acceptEdits` (+ optional `--max-budget-usd`), cwd at the worktree root. NO
      `--add-dir` — it grants *additional* dirs on top of an always-readable cwd,
      so it widens reach rather than confining it (see design.md §5). NOT
      `--permission-mode manual` — that prompts mid-turn and hangs in headless
      `-p`; the human gate is the confirm-diff (see design.md §5 note).
- [ ] 4.5 Apply-turn prompt identifies the comment by its **quoted code**, not its
      line number — anchors drift as earlier edits land — and instructs the turn
      to re-read the file first.
- [ ] 4.6 Reply turn: draft a rebuttal for **any** rejected comment, from Claude's
      justification where it has one and from the human's stated reason where the
      human overrode an `agree`/`unsure` verdict.
- [ ] 4.7 Assert verdict turns leave the worktree unmodified; assert apply turns
      compose (a second turn sees the first turn's edit); assert a verdict missing
      `depends_on` fails schema validation.
- [ ] 4.8 Pin and record the verified `claude` version (2.1.220). The envelope
      shape, `structured_output`, `--tools`, inline `--json-schema`, and
      resume-across-permission-modes are CLI surface, not a stable API — re-probe
      on upgrade rather than assuming (design.md §10).

## 5. Triage UI (`comment-triage`)

- [ ] 5.1 Run the verdict pass on load, one turn per file; hold verdicts alongside
      comments.
- [ ] 5.2 Layout: [file tree | document + comment | verdict + justification], tree
      toggleable with a keystroke.
- [ ] 5.3 Document pane: render the anchored document read-only with syntax
      highlighting and the comment's line/range highlighted in place. The document
      is polymorphic — source file, commit message (`/COMMIT_MSG`), or a synthetic
      change-overview (`/PATCHSET_LEVEL`).
- [ ] 5.4 File tree annotated per file with comment count + triage progress;
      `/COMMIT_MSG` and `/PATCHSET_LEVEL` appear as entries marked as non-files.
- [ ] 5.5 Keybindings: accept / reject / defer / fixed-by-hand / edit-prose /
      next / prev / next-undecided / toggle-tree. Record the per-comment decision
      (human decision overrides the verdict).
- [ ] 5.6 Edit-prose flow (via `minibuffer`) feeds the edited text into the later
      apply turn.
- [ ] 5.7 Surface each verdict's `depends_on` next to its justification; collapse
      the same fact declared by several verdicts into one shared dependency rather
      than repeating it per comment.
- [ ] 5.8 Reply drafting and approval happen **here**, at the end of triage — a
      draft offered for every rejected comment, each approved / edited / declined
      before finalize starts, so Phase 3 needs no human input.
- [ ] 5.9 Triage completion gate: an explicit action ends triage, reporting how
      many comments are still undecided; confirming the gate maps those to the
      skipped fate. Navigating past the last comment SHALL NOT end triage.
- [ ] 5.10 Detect on-disk changes to worktree files and re-read them (no conflict
      prompt — the pane is read-only). Invalidate any pending confirm-diff whose
      file changed underneath it.
- [ ] 5.11 Make the worktree path obtainable mid-review, for the
      fix-in-your-own-editor path.

## 6. Apply + confirm (`fix-application`)

- [ ] 6.1 After triage, iterate accepted comments with a real file anchor; issue a
      scoped apply turn per comment (preserving order within a file). Accepted
      `/COMMIT_MSG` comments go to 7.2 instead; `/PATCHSET_LEVEL` never gets an
      apply turn.
- [ ] 6.2 **Snapshot the file immediately before each apply turn.** This pre-turn
      snapshot — not the patchset baseline — is the reference for both the
      confirm-diff and the revert. For the first comment in a file the two are the
      same; for every later one they differ.
- [ ] 6.3 Render the confirm-diff (**pre-turn snapshot** vs edit) in the two
      panels; require explicit confirmation before the edit is kept. Diffing
      against the patchset would show earlier confirmed edits and hide what the
      reviewer is actually approving.
- [ ] 6.4 Reject → restore the pre-turn snapshot, then re-run the apply turn with
      an added hint, or skip the comment. **NOT `git checkout -- <file>`** — that
      restores the patchset baseline and destroys edits already confirmed for
      earlier comments in the same file.
- [ ] 6.5 On confirm, `git add` the file and keep the edit in the worktree;
      accumulate across comments; push nothing yet.
- [ ] 6.6 Regression test: confirm comment 1 on a file, apply then reject comment 2
      on the same file, assert comment 1's edit survives.

## 7. Finalize (`change-submission`)

- [ ] 7.1 Pre-push check: re-GET the change and abort unless `current_revision`
      still matches the revision checked out in the worktree. Report both
      revisions on mismatch; push nothing.
- [ ] 7.2 `git commit --amend` the staged edits into one commit, rewriting the
      commit message if a `/COMMIT_MSG` comment was accepted (so not a blanket
      `--amend --no-edit`); push to `refs/for/<branch>` with `<branch>` taken from
      the change's `branch` field — the worktree is on a detached HEAD with no
      local branch to read.
- [ ] 7.3 Push rejected → surface, keep worktree, do nothing else.
- [ ] 7.4 Push ok → issue the **single** batched review POST from 2.6 carrying
      every comment fate: accepted → `unresolved: false`; **fixed-by-hand →
      `unresolved: false`** (indistinguishable from accepted at this layer);
      rejected+replied → reply text + `unresolved: true`; skipped → omitted
      entirely. Replies were already approved in 5.8.

## 8. End-to-end verification

- [ ] 8.1 Manual run against a real test change: pull → verdict → triage → draft
      replies → apply → confirm → revision check → amend/push → batched review post.
- [ ] 8.2 Confirm the user's real working tree is never mutated across the whole
      flow.
- [ ] 8.3 Confirm nothing is resolved/replied when the push fails.
- [ ] 8.4 Confirm the batched review post actually resolves threads **on your
      Gerrit version** — the `unresolved: false` mechanism was verified only
      against a local stub.
- [ ] 8.5 Run `cargo test` in the worktree before the amend.

## 9. Follow-up design (not built here)

- [x] 9.1 Verify a `plan`-mode session can be resumed in an edit-capable mode
      (DONE — `acceptEdits`, context retained; see design.md §5/§12).
- [ ] 9.2 Tune the three prompts' prose and how much surrounding code each turn is
      shown (roles/scoping/granularity already settled in design.md §7).
- [ ] 9.3 Revisit auto modes (rung 1 / rung 2) once rung 0 is proven.
- [ ] 9.4 Consider batching a comment-dense file's comments into one apply turn if
      per-comment cost proves painful.
