## 1. Project scaffold

- [ ] 1.1 Initialize the Rust crate (`remendo`), edition 2024; set up CI (fmt +
      clippy + test) mirroring vybim's gates.
- [ ] 1.2 Add dependencies: `crossterm`, `ratatui`, `similar`, `serde` (derive),
      `serde_json`, and a blocking HTTP client (`ureq` + rustls). Confirm **no**
      `tokio`.
- [ ] 1.3 Copy `diff_view.rs` (+ its `Theme`/`git` seams) from vybim and adapt it
      to take an arbitrary before/after text pair (patchset vs edit), not just
      HEAD-vs-working-tree.

## 2. Gerrit client (`gerrit-client`)

- [ ] 2.1 Worker thread owning a blocking HTTP client; a `GerritEvent` channel to
      the UI. No async runtime.
- [ ] 2.2 Response helper that strips the `)]}'` XSSI prefix before `serde_json`.
- [ ] 2.3 Decide and wire the auth mechanism (cookie / HTTP password / .netrc).
- [ ] 2.4 Fetch: change + current revision + files; inline comments → filter
      unresolved → map to (file, line/range, author, message).
- [ ] 2.5 Mutations: mark-resolved and post-reply calls (used only in finalize).
- [ ] 2.6 Unit-test XSSI stripping and comment mapping against captured fixtures.

## 3. Workspace + patchset checkout (`review-workspace`)

- [ ] 3.1 `remendo <change-id>` entrypoint; error-and-exit on unknown/inaccessible
      change **before** any worktree is created.
- [ ] 3.2 Create a dedicated git worktree; `git fetch` the revision ref and check
      it out into it. Never touch the user's real working tree.
- [ ] 3.3 Abort path: leave the worktree + staged edits in place, push nothing,
      report the worktree location.

## 4. Claude driver (`claude-driver`)

- [ ] 4.1 Generate a session UUID per change; spawn-wait-parse wrapper around
      `claude -p` (the `git.rs` shape).
- [ ] 4.2 Verdict turn: `--permission-mode plan --output-format json
      --json-schema <verdict schema>`; parse validated `{verdict, justification,
      confidence}` per comment.
- [ ] 4.3 Apply turn: `--resume <uuid> --tools "Read,Edit" --permission-mode
      acceptEdits --add-dir <file dir>` (+ optional `--max-budget-usd`). NOT
      `--permission-mode manual` — that prompts mid-turn and hangs in headless
      `-p`; the human gate is the confirm-diff (see design.md §5 note).
- [ ] 4.4 Reply turn: draft a rebuttal from a rejected comment + Claude's
      justification.
- [ ] 4.5 Assert verdict turns leave the worktree unmodified; assert apply turns
      compose (a second turn sees the first turn's edit).

## 5. Triage UI (`comment-triage`)

- [ ] 5.1 Run the verdict pass on load; hold verdicts alongside comments.
- [ ] 5.2 Two-panel layout: [code + reviewer comment | verdict + justification].
- [ ] 5.3 Keybindings: accept / reject / edit-prose / next / prev. Record the
      per-comment decision (human decision overrides the verdict).
- [ ] 5.4 Edit-prose flow feeds the edited text into the later apply turn.

## 6. Apply + confirm (`fix-application`)

- [ ] 6.1 After triage, iterate accepted comments; issue a scoped apply turn per
      comment (preserving order within a file).
- [ ] 6.2 Render the confirm-diff (patchset baseline vs edit) in the two panels;
      require explicit confirmation before the edit is kept.
- [ ] 6.3 Reject → `git checkout -- <file>` to revert, then re-run the apply turn
      with an added hint, or skip the comment.
- [ ] 6.4 On confirm, keep the edit in the worktree; accumulate across comments;
      push nothing yet.

## 7. Finalize (`change-submission`)

- [ ] 7.1 `git commit --amend` all staged edits into one commit; push to
      `refs/for/<branch>`.
- [ ] 7.2 Push rejected → surface, keep worktree, do nothing else.
- [ ] 7.3 Push ok → batch-resolve accepted comments; batch-post approved replies
      for rejected comments (rejected+replied stay unresolved).

## 8. End-to-end verification

- [ ] 8.1 Manual run against a real test change: pull → verdict → triage → apply →
      confirm → amend/push → resolve/reply.
- [ ] 8.2 Confirm the user's real working tree is never mutated across the whole
      flow.
- [ ] 8.3 Confirm nothing is resolved/replied when the push fails.

## 9. Follow-up design (not built here)

- [x] 9.1 Verify a `plan`-mode session can be resumed in an edit-capable mode
      (DONE — `acceptEdits`, context retained; see design.md §5/§9).
- [ ] 9.2 Tune the three prompts' prose and how much surrounding code each turn is
      shown (roles/scoping/granularity already settled in design.md §7).
- [ ] 9.3 Revisit auto modes (rung 1 / rung 2) once rung 0 is proven.
- [ ] 9.4 Consider batching a comment-dense file's comments into one apply turn if
      per-comment cost proves painful.
