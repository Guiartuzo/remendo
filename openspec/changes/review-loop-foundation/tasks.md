> **Tiers 1–3 of `open-decisions.md` were settled on 2026-08-05** — Gerrit
> configuration and auth, worktree topology and relaunch, thread semantics,
> provenance, the verdict schema, and whether `depends_on.verify` is executed. The
> reasoning lives in `design.md` §13; the tasks below now state the decisions
> rather than defer to them.
>
> **Tier 4 (trait surfaces, error model) was settled on 2026-08-05** while
> building `gerrit-client`, since a trait's error type is part of its surface —
> see `design.md` §14. What remains open is Tier 5 (cost per change, which
> instruments itself via 4.9) and Tier 6.

## 1. Project scaffold

- [x] 1.1 Initialize the Rust crate (`remendo`), edition 2024; set up CI (fmt +
      clippy + test) mirroring vybim's gates.
- [x] 1.2 Add dependencies: `crossterm`, `ratatui`, `similar`, `ropey`, `ignore`,
      `tree-sitter` + `tree-sitter-highlight` + grammars, `serde` (derive),
      `serde_json`, and a blocking HTTP client (`ureq` + rustls). Confirm **no**
      `tokio`, and **no** `portable-pty`/`vt100` — v0 embeds no terminal.
- [x] 1.3 Copy `diff_view.rs` (+ its `Theme`/`git` seams) from vybim and adapt it
      to take an arbitrary before/after text pair (pre-turn snapshot vs edit), not
      just HEAD-vs-working-tree.
- [x] 1.4 Copy `theme.rs`, `syntax.rs`, and `buffer.rs` from vybim. `buffer.rs`
      ports whole (448 lines, no crate-internal deps); its mutation methods go
      unused in v0 but its file loading and line indexing are what the read-only
      viewer needs.
- [x] 1.5 Port the **read-only** slice of `pane.rs`: syntax-highlighted viewport
      render, scrolling (+ its existing viewport tests), `goto_line`, and search.
      Leave behind multi-caret, insert/delete/undo, word motion and completion —
      roughly two thirds of the module (design.md §11).
- [x] 1.6 Copy `file_tree.rs` (357 lines, depends only on `theme`) and
      `minibuffer.rs` (comment-prose input).
- [x] 1.7 Hold the **Ubuntu 20.04 / glibc 2.31 floor** in the dependency set: TLS
      through rustls only — never `native-tls`/`openssl`, which breaks across
      20.04's OpenSSL 1.1.1 and 24.04's 3.0. This constrains 1.2's choice of HTTP
      client; it is not a separate step but a veto over it.
- [x] 1.8 Release job: `cargo zigbuild --target x86_64-unknown-linux-gnu.2.31`
      plus the `objdump -T` guard that fails on any GLIBC symbol above 2.31.
      Lift both verbatim from vybim's `release.yml` (archived change
      `2026-07-09-linux-glibc-2-31-baseline`); pin Zig, it is pre-1.0. Needed
      only when the binary is distributed to the team — **not** to run 8.1 on
      your own box. Deferring it is fine; deferring 1.7 is not.

## 2. Gerrit client (`gerrit-client`)

- [x] 2.0 Define the `GerritApi` and `GitCli` traits and their fakes per
      **design.md §14**. `GerritApi` has **no `drafts` method** — excluding
      drafts was a decision, and a trait without the method cannot be talked
      into fetching them later. Credentials sit on `GitCli`, not `GerritApi`.
      Errors are per-module `thiserror` enums, each variant carrying the
      offending value.
- [x] 2.1 Worker thread owning a blocking HTTP client; a `GerritEvent` channel to
      the UI. No async runtime.
- [x] 2.2 Response helper that strips the `)]}'` XSSI prefix before `serde_json`.
      A body *without* the guard is an error, not a pass-through: Gerrit always
      emits it on the JSON API, so its absence means an HTML login/SSO page from
      a failed auth. The error quotes the body so it does not read as a parser
      bug.
- [x] 2.3 Auth via `git credential fill` for the resolved host — on the **git**
      trait, not the Gerrit one. No `.netrc` parsing, no credential file, no
      stored secret. Error names the host when no credential comes back.
- [x] 2.4 Resolve the Gerrit base URL from the clone's `origin` remote, with an
      explicit override always winning. Put the derived URL in the failure
      message — a subpath-hosted or SSH-remote Gerrit is not always derivable
      (design.md §13 table), and a hostless HTTP error is not actionable.
- [x] 2.5 TLS against the system root store (`rustls-native-certs`); on validation
      failure, point at `git config --get http.sslCAInfo`. A Gerrit that `git
      push` reaches but Remendo cannot is a trust-store difference, and reporting
      it as an auth failure costs a long detour.
- [x] 2.6 Fetch: change + `current_revision` + `branch` + files; inline comments →
      assemble into **threads** by `in_reply_to` chain → thread state is the
      **last** comment's `unresolved` flag (branch tiebreak: max `updated`) → map
      to (file, line/range, ordered comments with author + prose + id). Retain
      `current_revision` and `branch` — finalize needs both.
- [x] 2.7 Filter threads: exclude drafts (never fetch `/drafts`); **include** the
      user's own threads. Any self-authored filtering that is later added must be
      thread-level (threads you *started*) — filtering comments you wrote would
      drop your reply out of a reviewer's thread and take the thread's state with
      it.
- [x] 2.8 Fetch robot comments from `/robotcomments` — a **second endpoint with a
      distinct shape** (`robot_id`, `url`, `fix_suggestions`), not a filter flag.
      Model them as their own type; `fix_suggestions` is unused in v0 (see
      design.md §12 item 9) but must not be discarded at parse time.
- [x] 2.9 Drop threads anchored on a non-current patchset and **carry the count**
      out to the UI. Their line anchors address code several patchsets gone; a
      shorter queue must not be indistinguishable from a change with fewer
      comments.
- [x] 2.10 Classify comment anchors into a `CommentAnchor` enum: real file path,
      `/COMMIT_MSG` (commit message; resolve the line against the real message,
      accounting for Gerrit's synthetic Parent/Author/Commit header offset), or
      `/PATCHSET_LEVEL` (change-level, no path). Never derive an on-disk path or
      directory from the two pseudo-paths.
- [x] 2.11 Mutation: **one** batched review POST to
      `/changes/{id}/revisions/current/review` carrying every comment fate —
      accepted with `unresolved: false`, rejected-with-reply with
      `unresolved: true`. Each reply sets `in_reply_to` to its thread's **last**
      comment id, not its first. There is no mark-resolved endpoint; do not model
      one. Used only in finalize.
- [x] 2.12 Unit-test XSSI stripping, anchor classification (including the
      `/COMMIT_MSG` line offset), and comment mapping against captured fixtures.
      Add thread-assembly fixtures: a thread closed by a reply (must NOT be
      triaged), a three-comment thread whose live ask is the last comment, a
      branching thread resolved by `updated`, and a thread on an older patchset
      (must be skipped and counted).

## 3. Workspace + patchset checkout (`review-workspace`)

- [x] 3.1 `remendo <change-id>` entrypoint. Require cwd inside a git clone;
      error-and-exit on not-a-clone, on unknown/inaccessible change, and on a
      `project` mismatch between the change and the clone — the mismatch error
      names **both** values, per `config.yaml`'s error rule. All of this happens
      **before** any worktree is created.
- [x] 3.2 Create the worktree under `$XDG_STATE_HOME/remendo/<project>/<change-id>/`
      (outside the clone, so it survives `git clean`); `git fetch` the revision
      ref and check it out into it. Never touch the user's real working tree.
      Note git still registers the worktree in the clone's `.git/worktrees/`, so
      a hand-deleted state dir needs `git worktree prune`.
      **REFINED while building:** that directory is a *session* directory
      holding `worktree/` and `verdicts.json`, not the worktree itself. The
      cache must sit beside the checkout rather than inside it — anything inside
      would appear as an untracked file in the change under review.
      Project and change id are validated as path components before use: a
      Gerrit project legitimately contains `/` and becomes nested directories,
      but `..` must be impossible, since the project name arrives from a REST
      response.
- [x] 3.3 Relaunch onto an existing worktree is a **resume**: reuse it with its
      confirmed edits intact. Do not recreate, do not refuse — 3.4's abort path
      guarantees this is the common case, not an exceptional one.
- [x] 3.4 Abort path: leave the worktree + confirmed edits in place, push nothing,
      report the worktree location.
- [x] 3.5 Verdict cache keyed by **(change id, revision)**, persisted beside the
      worktree, holding verdict payloads and accumulated `total_cost_usd`. A
      revision change is a cache miss. Human triage decisions are deliberately
      NOT cached — cheap to redo, where verdicts cost money and re-roll
      differently over a worktree that already holds round-one fixes.

## 4. Claude driver (`claude-driver`)

- [ ] 4.1 Generate a session UUID per change; spawn-wait-parse wrapper around
      `claude -p` (the `git.rs` shape), behind the project's own driver trait.
      **The trait, `Envelope`, `SessionId` and `FakeDriver` already exist** —
      built during §6, which needed something to apply edits through. What
      remains here is the REAL implementation that spawns `claude`, and that is
      what 4.11's gate covers.
- [ ] 4.2 Envelope decoding: `--output-format json` returns a **result envelope**,
      not the payload. Deserialize it, gate on `is_error`, and read the payload
      from `structured_output` (a native object) — not from `result` (the same
      JSON as a string). Do NOT gate on `stop_reason == "end_turn"`; the
      structured-output path returns `tool_use`. Cover with a named fake
      implementing the driver trait, per `config.yaml`'s mock-external-IO rule.
- [ ] 4.3 Verdict schema + turn: `--permission-mode plan --output-format json
      --json-schema <schema JSON inline — a file path is rejected>`. Issue **one
      turn per file**, not one over the whole change. Concrete schema:

      ```jsonc
      {
        "comment_id":    "string",
        "verdict":       "agree" | "disagree" | "unsure",
        "justification": "string",
        "depends_on":    null | [ {
            "fact":     "what is unknown",
            "kind":     "ci-config" | "tool-version" | "team-convention"
                      | "roadmap" | "ticket" | "other",
            "verify":   "how a HUMAN could settle it",
            "flips_to": "agree" | "disagree" | "unsure" | null
        } ]
      }
      ```

      `depends_on` is **required, nullable, never omittable**, and an **array** —
      one verdict can rest on several facts and the dry run's 12 verdicts
      collapsed onto 10 facts. Aggregation by `fact` happens in the app layer
      (5.7), not the schema. **No `confidence` field** — 9 of 12 context-dependent
      verdicts were filed as confident, so the grade pointed away from what the
      human needs; `depends_on` is the checkable signal instead.
- [ ] 4.3b Verdict prompt instructs the turn to read the recorded out-of-code
      context first — `openspec/config.yaml`, `CLAUDE.md`, CI workflow files —
      using the `Read` its `plan` mode already grants. Settles the dependencies a
      file read can settle before they are ever declared, so `depends_on` carries
      only the residue that needs a human.
- [ ] 4.3c Verdict prompt feeds the **whole thread**, not the opening comment.
      Anchor from the first comment, question from the full exchange. Author names
      stay out of the verdict input (they are UI-only, design.md §7).
- [ ] 4.4 Apply turn: `--resume <uuid> --tools "Read,Edit" --permission-mode
      acceptEdits` (+ optional `--max-budget-usd`), cwd at the worktree root. NO
      `--add-dir` — it grants *additional* dirs on top of an always-readable cwd,
      so it widens reach rather than confining it (see design.md §5). NOT
      `--permission-mode manual` — that prompts mid-turn and hangs in headless
      `-p`; the human gate is the confirm-diff (see design.md §5 note).
- [ ] 4.5 Apply-turn prompt identifies the comment by its **quoted code**, not its
      line number — anchors drift as earlier edits land — and instructs the turn
      to re-read the file first.
- [ ] 4.6 Reply turn: draft a rebuttal for **any** rejected thread — including
      robot threads — from Claude's justification where it has one and from the
      human's stated reason where the human overrode an `agree`/`unsure` verdict.
- [ ] 4.7 Assert verdict turns leave the worktree unmodified; assert apply turns
      compose (a second turn sees the first turn's edit); assert a verdict missing
      `depends_on` fails schema validation; assert a verdict carrying a
      `confidence` field is rejected by the schema rather than silently accepted.
- [ ] 4.9 Accumulate `total_cost_usd` from every envelope into the per-change
      cache (3.5) and display the running total. This is what settles
      `open-decisions.md` Tier 5 — one real run prices a change permanently, and
      robot comments in scope (2.8) make the number less predictable than the
      original estimate assumed.
- [ ] 4.10 **Never execute a `depends_on.verify` string.** It is prose rendered to
      the human. Executing it would reintroduce arbitrary command execution from
      model output directly after 4.4 removed shell access — and the dry run's one
      measured self-clearing dependency was a probe invoking `claude` itself, so
      auto-running it would silently spawn billable subprocesses. No allowlist, no
      confirm gate, no code path that could run one in v0.
- [ ] 4.11 **GATE ON §4 — do this first, before 4.1.** Deferred deliberately on
      2026-08-05 (re-probing costs money and blocks nothing until §4), but §4
      may not start until it is done.
      Pin and record the verified `claude` version. Everything was probed
      against **2.1.220**; the dev box is now on **2.1.222**, so per this project's
      own rule the pin is stale. Re-probe the envelope shape,
      `structured_output`, `--tools`, inline `--json-schema`, and
      resume-across-permission-modes, then move the pin — or pin the box back.
      These are CLI surface, not a stable API (design.md §10).

## 5. Triage UI (`comment-triage`)

- [ ] 5.1 Run the verdict pass on load, one turn per file; hold verdicts alongside
      threads. Load from the (change, revision) cache (3.5) when it hits, so a
      relaunch does not re-spend or re-roll the pass.
- [ ] 5.1b Report the count of threads skipped for patchset age (2.9) at load,
      before triage begins. A queue shorter than the change's unresolved count
      must not read as a change with fewer comments.
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
- [ ] 5.7 Surface each verdict's `depends_on` entries next to its justification —
      `fact`, `verify`, and `flips_to` — collapsing the same fact declared by
      several verdicts into one shared dependency rather than repeating it per
      comment. With no `confidence` field, these entries **are** the uncertainty
      signal. Render `verify` as text with **no action that runs it** (4.10).
- [ ] 5.7b Document pane shows the **whole thread** in order with authors, and one
      decision is recorded for the thread. The live ask is often the last comment,
      not the first.
- [ ] 5.8 Reply drafting and approval happen **here**, at the end of triage — a
      draft offered for every rejected thread including robot threads, each
      approved / edited / declined before finalize starts, so Phase 3 needs no
      human input.
- [ ] 5.9 Triage completion gate: an explicit action ends triage, reporting how
      many comments are still undecided; confirming the gate maps those to the
      skipped fate. Navigating past the last comment SHALL NOT end triage.
- [ ] 5.10 Detect on-disk changes to worktree files and re-read them (no conflict
      prompt — the pane is read-only). Invalidate any pending confirm-diff whose
      file changed underneath it.
- [ ] 5.11 Make the worktree path obtainable mid-review, for the
      fix-in-your-own-editor path.

## 6. Apply + confirm (`fix-application`)

- [x] 6.1 After triage, iterate accepted comments with a real file anchor; issue a
      scoped apply turn per comment (preserving order within a file). Accepted
      `/COMMIT_MSG` comments go to 7.2 instead; `/PATCHSET_LEVEL` never gets an
      apply turn.
- [x] 6.2 **Snapshot the file immediately before each apply turn.** This pre-turn
      snapshot — not the patchset baseline — is the reference for both the
      confirm-diff and the revert. For the first comment in a file the two are the
      same; for every later one they differ.
- [x] 6.3 Render the confirm-diff (**pre-turn snapshot** vs edit) in the two
      panels; require explicit confirmation before the edit is kept. Diffing
      against the patchset would show earlier confirmed edits and hide what the
      reviewer is actually approving.
- [x] 6.4 Reject → restore the pre-turn snapshot, then re-run the apply turn with
      an added hint, or skip the comment. **NOT `git checkout -- <file>`** — that
      restores the patchset baseline and destroys edits already confirmed for
      earlier comments in the same file.
- [x] 6.5 On confirm, `git add` the file and keep the edit in the worktree;
      accumulate across comments; push nothing yet.
- [x] 6.6 Regression test: confirm comment 1 on a file, apply then reject comment 2
      on the same file, assert comment 1's edit survives.

## 7. Finalize (`change-submission`)

- [x] 7.1 Pre-push check: re-GET the change and abort unless `current_revision`
      still matches the revision checked out in the worktree. Report both
      revisions on mismatch; push nothing.
- [x] 7.2 `git commit --amend` the staged edits into one commit, rewriting the
      commit message if a `/COMMIT_MSG` comment was accepted (so not a blanket
      `--amend --no-edit`); push to `refs/for/<branch>` with `<branch>` taken from
      the change's `branch` field — the worktree is on a detached HEAD with no
      local branch to read.
- [x] 7.3 Push rejected → surface, keep worktree, do nothing else.
- [x] 7.4 Push ok → issue the **single** batched review POST from 2.6 carrying
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
- [ ] 8.6 On a real Ubuntu 20.04 box, confirm the **runtime dependencies** work,
      not just that the binary starts: `claude` 2.1.x runs there at all, and git
      2.25.1 supports the worktree/fetch/amend/push sequence §3 and §7 rely on.
      A glibc-2.31 binary that shells out to a `claude` which will not start on
      20.04 is still unusable — and zigbuild cannot fix that.

## 9. Follow-up design (not built here)

- [x] 9.1 Verify a `plan`-mode session can be resumed in an edit-capable mode
      (DONE — `acceptEdits`, context retained; see design.md §5/§12).
- [ ] 9.2 Tune the three prompts' prose and how much surrounding code each turn is
      shown (roles/scoping/granularity already settled in design.md §7).
- [ ] 9.3 Revisit auto modes (rung 1 / rung 2) once rung 0 is proven.
- [ ] 9.4 Consider batching a comment-dense file's comments into one apply turn if
      per-comment cost proves painful.
