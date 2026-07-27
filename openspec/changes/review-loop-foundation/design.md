# Design — Remendo review-loop foundation

This document captures the architecture decisions settled during exploration.
It is the rationale behind `proposal.md` and the `specs/` deltas.

## 1. Product identity: a sibling, not a mode

Remendo reuses ~80% of what vybim *is* — "render things in a terminal + drive
subprocesses" — and almost none of the remaining 20% ("edit text"). But an
editor and a review cockpit have opposite defaults:

```
   vybim (editor)                    remendo (review cockpit)
   ──────────────                    ────────────────────────
   blank buffer + cursor             a change loaded, comments queued
   editing is the point              editing is the rare escape hatch
   buffer.rs is the CENTER           diff_view.rs is the CENTER
   subprocess = LSP servers          subprocess = Claude Code + git + Gerrit
```

Cramming both into one binary's startup, keymap, and focus model (vybim's
`app.rs` is already ~2800 lines) risks a product that is mediocre at both. So
Remendo is a **new repo**.

**Reuse strategy: copy and diverge.** vybim is a *binary* with no `lib.rs`, and
its `diff_view.rs` is coupled to `crate::git` and `crate::theme`. Extracting a
clean shared crate is real work, and the two diff renderers are about to grow
apart anyway (editor diff = HEAD vs working tree; review diff = patchset vs
Claude's edit). So we copy the useful modules into Remendo and let them diverge.
Cost: a bug fixed in one does not reach the other. Accepted for v0; extract a
`vybim-core` crate later, only when we notice we are fixing the same bug twice.

What gets copied:
- `diff_view.rs` (+ the `similar` dependency) → the confirm-diff surface.
- `git.rs`'s spawn-wait-parse shape → git operations and Claude one-shots.
- `lsp/transport.rs`'s worker-thread + `AppEvent`-channel shape → Gerrit access.

## 2. The reframe that shaped everything: Claude reviews the reviewer

The original sketch assumed AI-*generated* review comments (Gerrit robot
comments, `robot_id`, `fix_suggestions`). Reality on our team is the opposite:

```
   ORIGINAL ASSUMPTION                 ACTUAL REALITY
   ───────────────────                 ──────────────
   AI writes the review comments       Humans write the review comments
   humans read/accept them             AI (Claude) applies them
   → robot_id, fix_suggestions          → plain inline comments, prose,
     structured suggested edits           from N different dev accounts
```

Consequences, all load-bearing:
- **`fix_suggestions` is gone.** Human comments are free-text prose ("this is
  O(n²)", "rename tmp", "extract a helper"). There is no structured edit to
  apply — Claude must interpret intent from prose. This is *harder* but also
  *more valuable*: applying a pre-structured suggestion is a one-click thing
  Gerrit already does; interpreting vague prose is the tedious part humans hate.
- **Claude gets a first job: adjudicate.** Because reviewers can be wrong, the
  first pass is a **verdict** (agree / disagree / unsure + justification) on each
  comment — not an edit. Claude becomes a second reviewer of the reviewer.
- **We build the apply step ourselves** via Claude; we do not hook Gerrit's
  suggested-edit machinery.

## 3. The loop: two-phase, manual (rung 0)

Reading prose comments is fast; Claude edit-generation is slow. So we separate a
fast human-judgment phase from a supervised machine phase, rather than
interleaving them one comment at a time.

```
 ┌─ SETUP ──────────────────────────────────────────────────────────┐
 │  remendo 12345                                                     │
 │    GET change + current revision + files                          │
 │    git worktree add + git fetch refs/changes/45/12345/<ps> +      │
 │      checkout  (isolated; user's real tree untouched)             │
 │    GET comments → filter unresolved → map to (file, line, prose)  │
 │    generate a session UUID; spawn the verdict turn (plan mode)    │
 └───────────────────────────────┬──────────────────────────────────┘
                                  ▼
 ┌─ PHASE 1 · TRIAGE  (human, informed by Claude's verdicts) ───────┐
 │  two panels:  [ code + reviewer comment | Claude verdict + why ] │
 │  per comment:  a accept · r reject · e edit-the-prose · n/p nav  │
 │  nothing is written; verdicts already computed in SETUP          │
 └───────────────────────────────┬──────────────────────────────────┘
                                  ▼
 ┌─ PHASE 2 · APPLY  (per COMMENT, resumed Claude session) ─────────┐
 │  for each accepted comment (order within a file preserved):      │
 │    claude -p --resume <uuid> --tools "Read,Edit"                 │
 │      --permission-mode acceptEdits --add-dir <file's dir>       │
 │    → CONFIRM DIFF (patchset baseline vs edit) in the same panels │
 │    → confirm: keep the edit in the worktree (NOT pushed)        │
 │    → reject: revert the file, then re-run with a hint or skip    │
 └───────────────────────────────┬──────────────────────────────────┘
                                  ▼
 ┌─ PHASE 3 · FINALIZE  (once, git-native) ─────────────────────────┐
 │  git commit --amend   (all confirmed edits → one new patchset)    │
 │  git push <gerrit> HEAD:refs/for/<branch>                         │
 │    push rejected? → surface, keep the worktree, bail             │
 │  push ok → resolve accepted comments (batch)                    │
 │          → post approved replies for rejected comments (batch)   │
 └───────────────────────────────────────────────────────────────────┘
 ABORT at any point → leave the worktree in place; nothing pushed.
```

### Why two-phase over interleaved
Interleaving makes machine work interrupt the human's reading flow one comment
at a time (context-switch tax). Two-phase lets the reviewer blaze through triage
uninterrupted, then supervise a stack of diffs. The whole loop is uniformly
per-comment (verdict → triage → apply → confirm → resolve), so there is no
"file batch" concept to track — see §7 "Prompt contract".

## 4. Talking to Gerrit: worker thread, no async

vybim's grain is "shell out / use background threads + channels; the render loop
is synchronous." We keep it. Gerrit REST runs on a background worker owning a
**blocking** HTTP client (`ureq`-style), delivering `GerritEvent`s to the UI over
a channel — the same shape as `lsp/transport.rs`. We deliberately **reject
`tokio`**: grafting an async runtime onto a synchronous TUI for one REST client
is the tail wagging the dog.

Getting-back-to-Gerrit uses **amend-and-push a new patchset**, not the
change-edit REST API. Rationale: the tool owns a real checkout, our devs are
git-native, amend-and-push is how a human would do it by hand, and it batches all
confirmed fixes into one unit. (The change-edit API only looked right back when
we assumed no local checkout.)

Gotcha captured: Gerrit prefixes REST responses with a `)]}'` XSSI guard line
that must be stripped before `serde_json`.

## 5. Driving Claude Code

Verified empirically against `claude` v2.1.220. Two supported mechanisms:

```
  A) LIVE STREAMING SESSION                B) RESUME-BASED ONE-SHOTS  ← v0 choice
     claude -p --input-format stream-json     claude -p --session-id U ...  (turn 1)
       --output-format stream-json            claude -p --resume U ...      (turn N)
     one long-lived process                   N clean spawn-wait-parse calls
     maps onto lsp/transport.rs               maps onto git.rs
```

**v0 uses B.** Each turn is a clean spawn-wait-parse — the `git.rs` shape, far
simpler to build correctly than long-lived bidirectional process management (no
stdin-keepalive, backpressure, or partial-frame handling). Graduate to A only if
latency/cost demands it. Both mechanisms exist in vybim's source as patterns.

**Composition is safe regardless of A vs B.** Edits compose because the *working
tree persists* between turns (the worktree is the source of truth) — turn 2 reads
the file turn 1 wrote. Session resume merely keeps Claude's reasoning coherent on
top of that. So the classic contradiction ("rename x" + "delete the block using
x") cannot occur.

### Blast radius is enforced by flags, not by hoping in the prompt
The "'rename this' becomes a 12-file refactor" risk is partly *structural*:
- verdict pass: `--permission-mode plan` → Claude analyzes but **cannot edit**.
- apply pass: `--tools "Read,Edit"` (no Bash/shell escapes),
  `--add-dir <the file's dir>` to scope filesystem reach,
  `--permission-mode acceptEdits` (see note below), `--max-budget-usd` ceiling.
- structured verdicts: `--output-format json --json-schema` returns validated
  `{verdict, justification, confidence}` — the triage panel consumes data, not
  parsed prose.

The remaining, non-structural blast-radius guard is the **confirm-diff** in
Phase 2, whose reject → re-run-with-a-hint edge is where a too-broad edit gets
reined in.

> **Permission-mode note (verified against `claude` v2.1.220).** Do not confuse
> Claude's `--permission-mode manual` with our "rung 0 manual" gate. `manual`
> makes Claude prompt for tool permission *mid-turn* — in headless `-p` there is
> no TTY to answer, so apply turns must instead run with `--permission-mode
> acceptEdits` (edits auto-allowed, still constrained by `--tools`/`--add-dir`).
> Our rung-0 human control is the **confirm-diff after the turn**, not a Claude
> permission prompt during it. Verified end to end: a `plan`-mode verdict turn
> made no edits, then the *same session* resumed in `acceptEdits` edited the
> file **and retained turn-1 context** (renamed a symbol it was never re-told the
> name of). This is the assumption the whole shared-session flow rests on.

## 6. Two verdicts per comment, and the reply feature

Each comment carries two independent signals — Claude's verdict and the human's
decision. The disagreement cells are the interesting ones:

```
                   Claude AGREES          Claude DISAGREES
                ┌─────────────────────┬──────────────────────────┐
  You ACCEPT    │ easy: apply the fix │ you override Claude → apply│
                ├─────────────────────┼──────────────────────────┤
  You REJECT    │ you override Claude   │ ★ both reject → the       │
                │ → skip               │   reviewer was wrong       │
                └─────────────────────┴──────────────────────────┘
```

The ★ cell is a v0 feature: when you *and* Claude reject a comment, Claude
already wrote the justification, so it **drafts a reply to the reviewer** that you
approve and post — closing the loop on "reviewers can be wrong" without typing a
rebuttal.

Comment fates, and how they interact with resolve-at-push:

```
  ACCEPTED + applied   → edit lands in the amend  → mark RESOLVED (after push)
  REJECTED + reply     → no edit; approved rebuttal → stay UNRESOLVED, POST reply
   (incl. ★ cell)                                     (ball is in reviewer's court)
  SKIPPED              → nothing                    → left as-is
```

## 7. Prompt contract

One shared session per change (§5) means the expensive context is established
once and inherited; the only new information each later turn injects is **the
human's decisions**, which Claude never saw happen.

```
  TURN 1 · VERDICT (plan mode)
    feed:  change subject/message · all comments (file+line+author+prose)
    Claude uses Read/Grep to inspect the real code (cannot edit)
    emits: {comment_id, verdict, justification, confidence}  (schema-validated)
    session now holds ──▶ [ the code it read ][ all comments ][ its verdicts ]
                                          │
        ── human triages (accept/reject/edit) — Claude is BLIND to this ──
                                          │
  TURN 2..M · APPLY (resume, one turn PER COMMENT)   ▼
    can ASSUME:   Claude still remembers the code, comments, its verdicts
    must RESTATE: the ONE accepted comment (+ any human-edited prose) + "apply now"
                                          │
  TURN (reply) · REPLY (resume)           ▼
    can ASSUME:   Claude remembers its justification per comment
    must RESTATE: which comment was rejected → "draft a reply"
```

### Verdict prompt
Adjudicates the **underlying concern**, not the reviewer's literal suggestion —
so a comment that is right about the problem but wrong about the fix still reads
as `agree`, with the nuance in the justification.
- `agree` = a real issue here worth addressing in this change.
- `disagree` = no real issue / moot / not worth it (the reviewer was wrong).
- `unsure` = needs human judgment, **including** when the comment depends on
  context Claude cannot see from the code (roadmap, past incidents, conventions).
- Judged on **technical merit**. The comment **author's name is shown in the UI**
  so the human can weigh who said it, but the author is **not** fed to Claude and
  does not sway the verdict (no reviewer-profile config in v0).
- Justification is reviewer-facing quality — it doubles as ammunition for the
  reply prompt.
- Runs in `plan` mode with `--output-format json --json-schema`.

### Apply prompt (per comment)
- **Concern-driven**: address what the comment is really about, treating its
  wording as a strong hint, not gospel — Claude will not knowingly apply a fix it
  judged wrong. Leash: *make the smallest change that fully resolves the concern;
  do not refactor beyond it; edit only this file; re-read the file first.*
- One turn per accepted comment (see §7 rationale below). Edits **compose**
  because each turn reads the worktree the previous confirmed turn wrote (§5).
- Scoped by flags: `--tools "Read,Edit"`, `--add-dir <file's dir>`,
  `--permission-mode acceptEdits` (headless auto-allow; the human gate is the
  confirm-diff, not a Claude prompt — see §5 note), optional `--max-budget-usd`.
- **Reject = a file revert.** Claude's Edit writes to the worktree directly, so
  the confirm-diff is *patchset baseline vs. current worktree file*; rejecting
  means `git checkout -- <file>` to restore the baseline, then re-run with a hint
  (same session — Claude remembers its attempt) or skip.

### Reply prompt
- Scoped to the **both-reject (★) cell**, where Claude has real ammunition (its
  own justification). Respectful, concise, collegial, cites the specific reason,
  no condescension. Output `{reply}`; the human approves before it is posted.

### Why per-comment (locked)
Cost scales as *accepted-comments / files-touched* — zero difference on
single-comment files, ~linear only on comment-dense ones. In exchange, the loop
becomes uniformly per-comment with clean 1:1 attribution (each confirm-diff and
each resolve maps to exactly one comment) and no "file batch" concept. The old
"per-comment edits stomp each other" objection does **not** apply here: the
shared session applies turns sequentially to the same worktree, so they compose.
Cap runaway cost with `--max-budget-usd`; batching a dense file's comments into
one turn is a localized optimization deferred to later.

## 8. Settled decisions (quick reference)

- **Resolve/reply timing**: only after a successful push, batched. A resolution
  or "fixed" reply with no patchset behind it would be a lie.
- **Abort**: isolate, don't stash. Review happens in a dedicated worktree; abort
  leaves it in place (resumable / removable). The user's real tree is never
  mutated, so there is no WIP to protect.
- **Accumulation**: confirmed-but-unpushed edits pile up in the worktree between
  per-comment confirms; one amend + push at the end folds them together.
- **Auto mode**: dial, not switch (rung 0 manual / rung 1 semi / rung 2 full).
  v0 ships rung 0 only.
- **Apply granularity**: **per comment** (locked, §7) — the loop is uniformly
  per-comment.

## 9. Open threads (next design tasks, not part of this change's build)

> ⚠ **STRATEGIC GO/NO-GO — decide before building anything (next session).**
> Do we build the ratatui TUI at all, or drive this loop **natively inside a
> Claude Code session**? Most of what this change designs is what Claude Code
> already *is* — an interactive terminal session that reasons, proposes edits,
> shows diffs, and gates on permission (rung-0 confirm). The whole `claude-driver`
> capability (plan→resume→acceptEdits, session UUIDs, structured-output parsing)
> exists **only because we assumed Claude is a subprocess**; if Claude Code is the
> host, it evaporates. Options:
> - **Z0** pure Claude Code + a Gerrit MCP/skill + a "review this change" prompt.
> - **Z1** Z0 + a skill encoding the loop (the three prompts become its content).
> - **Z2** a thin CLI that sets up the worktree + launches `claude`.
> - **Z3** the full TUI as designed here.
>
> The TUI (Z3) only adds a spatial two-pane UI, single-keystroke triage,
> deterministic orchestration, and non-Claude-Code users — all *UX optimizations
> over a proven core*, not the value. **Leaning: prototype Z1 first** (the Gerrit
> access layer is needed in every option and is 100% reusable), use it on real
> reviews, build the TUI only if conversational triage hurts at volume. Almost
> nothing designed here is wasted either way — only the ratatui UI and
> `claude-driver` are premature if we go Z1. The hinge is whether conversational
> UX can handle the review volume that started this project.

1. ~~Resume-across-permission-modes check.~~ **DONE** (verified against `claude`
   v2.1.220): a `plan` verdict session resumes in `acceptEdits`, edits, and keeps
   turn-1 context. This also corrected apply from `--permission-mode manual` to
   `acceptEdits` — see the §5 permission-mode note.
2. **Prompt prose tuning.** Roles, scoping flags, I/O contracts, and granularity
   are settled (§7); the remaining task is the exact wording and how much
   surrounding code each turn is shown.
3. **Gerrit auth mechanism** (cookie / HTTP password / .netrc) — pin during
   implementation of `gerrit-client`.
4. **Resumable review sessions** (worktree + a small state file) — a v1 nicety
   the worktree model already makes cheap.
