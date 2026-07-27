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
 │  verdicts already computed in SETUP; depends_on shown per verdict│
 │  nothing is written to the worktree                              │
 └───────────────────────────────┬──────────────────────────────────┘
                                  ▼
 ┌─ PHASE 2 · APPLY  (per COMMENT, resumed Claude session) ─────────┐
 │  for each accepted comment (order within a file preserved):      │
 │    SNAPSHOT the file  ← the confirm/revert reference point       │
 │    claude -p --resume <uuid> --tools "Read,Edit"                 │
 │      --permission-mode acceptEdits                              │
 │    → CONFIRM DIFF (pre-turn snapshot vs edit) in the same panels │
 │    → confirm: git add; keep the edit in the worktree (NOT pushed)│
 │    → reject: restore the snapshot, re-run with a hint or skip    │
 │  then, still attended: DRAFT + APPROVE a reply per rejected      │
 │  comment, so Phase 3 needs no human input                        │
 └───────────────────────────────┬──────────────────────────────────┘
                                  ▼
 ┌─ PHASE 3 · FINALIZE  (once, unattended, git-native) ─────────────┐
 │  re-GET change → current_revision still ours? else ABORT         │
 │  git commit --amend   (all confirmed edits → one new patchset)    │
 │    (+ rewrite the message if a /COMMIT_MSG comment was accepted) │
 │  git push <gerrit> HEAD:refs/for/<change.branch>                 │
 │    push rejected? → surface, keep the worktree, bail             │
 │  push ok → ONE review POST: accepted → unresolved:false          │
 │                             rejected → reply + unresolved:true   │
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
  `--permission-mode acceptEdits` (see note below), `--max-budget-usd` ceiling.
- structured verdicts: `--output-format json --json-schema` returns validated
  `{verdict, justification, confidence, depends_on}` — the triage panel consumes
  data, not parsed prose.

> **`--add-dir` does not confine (verified against `claude` v2.1.220).** An
> earlier draft of this design scoped apply turns with `--add-dir <the file's
> dir>` and called it a structural guard. It is the opposite: `--add-dir` grants
> *additional* directories on top of a working directory that stays fully
> readable, so passing it grants strictly **more** reach than omitting it. Probed
> directly — cwd at the worktree root, `--add-dir <wt>/src/a`, asked for
> `src/b/g.rs`; it read the file in neither directory. Confining by setting cwd to
> the file's directory instead was considered and rejected: apply turns need to
> re-read neighbouring code, and the worktree root is where git operates.
> So apply turns run at the worktree root with no `--add-dir`, and the honest
> guards are the tool restriction plus the confirm-diff.

The remaining, non-structural blast-radius guard is the **confirm-diff** in
Phase 2, whose reject → re-run-with-a-hint edge is where a too-broad edit gets
reined in. With confinement off the table, this is doing more of the work than
the original framing implied.

### The JSON envelope is not the payload
`--output-format json` returns a *result envelope* — `is_error`, `session_id`,
`usage`, `total_cost_usd`, `stop_reason`, and the payload nested inside. Parsing
it directly into a verdict cannot work. Verified against v2.1.220 with a verdict
schema attached:

```
{"is_error":false, ..., "stop_reason":"tool_use",
 "result":"{\"verdict\":\"disagree\",\"justification\":\"...\"}",
 "structured_output":{"verdict":"disagree","justification":"..."}}
```

Three consequences for `claude-driver`:
- Read `structured_output` (a native object), **not** `result` (the same JSON as
  a string, needing a second decode).
- Gate on `is_error`. Do **not** gate on `stop_reason == "end_turn"` — the
  structured-output path returned `tool_use` with `num_turns: 2`.
- `--json-schema` takes the schema **inline**, not a file path; passing a path
  fails with `--json-schema is not valid JSON`.

> **Permission-mode note (verified against `claude` v2.1.220).** Do not confuse
> Claude's `--permission-mode manual` with our "rung 0 manual" gate. `manual`
> makes Claude prompt for tool permission *mid-turn* — in headless `-p` there is
> no TTY to answer, so apply turns must instead run with `--permission-mode
> acceptEdits` (edits auto-allowed, still constrained by `--tools` — but not by
> `--add-dir`, see the note above).
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

The ★ cell is where the reply feature is *strongest* — you and Claude agree the
reviewer was wrong, and Claude already wrote the justification, so it **drafts a
reply** you approve and post. But drafting is offered for **every** rejection, not
just ★: a rejection against an `agree`/`unsure` verdict means you hold context
Claude lacked, which is often the rebuttal most worth sending (§7).

Comment fates. Note that "resolve" is not a separate operation — every fate below
is one entry in the same batched review post, differing only in `unresolved`:

```
  ACCEPTED + applied   → edit lands in the amend  → reply + unresolved:FALSE
  REJECTED + reply     → no edit; approved rebuttal → reply + unresolved:TRUE
   (any cell, not just ★)                            (ball is in reviewer's court)
  SKIPPED              → nothing                    → omitted from the post
```

## 7. Prompt contract

One shared session per change (§5) means the expensive context is established
once and inherited; the only new information each later turn injects is **the
human's decisions**, which Claude never saw happen.

```
  TURN 1..F · VERDICT (plan mode, ONE TURN PER FILE)
    feed:  change subject/message · that file's comments (line+author+prose)
    Claude uses Read/Grep to inspect the real code (cannot edit)
    emits: {comment_id, verdict, justification, confidence, depends_on}
           (schema-validated; depends_on required, nullable)
    session now holds ──▶ [ the code it read ][ all comments ][ its verdicts ]
                                          │
        ── human triages (accept/reject/edit) — Claude is BLIND to this ──
        ── replies for rejected comments are drafted + approved HERE ──
                                          │
  TURN F+1.. · APPLY (resume, one turn PER COMMENT)  ▼
    can ASSUME:   Claude still remembers the code, comments, its verdicts
    must RESTATE: the ONE accepted comment (+ any human-edited prose), the code
                  it QUOTED (not its line number — anchors drift), + "apply now"
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
- **`depends_on` is required** (nullable, not omittable): the out-of-code fact the
  verdict rests on — CI config, tool version, team convention, roadmap, ticket
  number — or `null` when the code alone settles it. See §9.
- Runs in `plan` mode with `--output-format json --json-schema`, **one turn per
  file**. A single turn over a 40-comment change returns 40 adjudications after
  reading five files, which is where truncation and blanket "looks fine" verdicts
  appear; per-file chunking bounds the payload and the shared session keeps the
  cross-file context anyway.

### Apply prompt (per comment)
- **Concern-driven**: address what the comment is really about, treating its
  wording as a strong hint, not gospel — Claude will not knowingly apply a fix it
  judged wrong. Leash: *make the smallest change that fully resolves the concern;
  do not refactor beyond it; edit only this file; re-read the file first.*
- One turn per accepted comment (see §7 rationale below). Edits **compose**
  because each turn reads the worktree the previous confirmed turn wrote (§5).
- **Identify the comment by its quoted code, not its line number.** Anchors are
  `(file, line)` against the patchset, and every earlier confirmed edit shifts
  them; after three edits to one file in the dry run all four anchors were stale.
  Feed the comment's quoted context and have the turn re-read the file. Resolution
  keys off the comment id, so finalize is unaffected.
- Scoped by flags: `--tools "Read,Edit"`, `--permission-mode acceptEdits`
  (headless auto-allow; the human gate is the confirm-diff, not a Claude prompt —
  see §5 note), optional `--max-budget-usd`. No `--add-dir` — it widens rather
  than confines (§5).
- **Reject = restore the pre-turn snapshot.** Claude's Edit writes to the worktree
  directly, so each turn is bracketed by a snapshot taken immediately before it:
  the confirm-diff is *pre-turn snapshot vs. current worktree file*, and rejecting
  restores that snapshot, then re-runs with a hint (same session — Claude
  remembers its attempt) or skips.
  **Not `git checkout -- <file>`.** That restores the *patchset* baseline, which
  is only the same thing for the first comment in a file; for any later comment it
  silently destroys the edits already confirmed for earlier comments. Same reason
  the confirm-diff is snapshot-based: diffed against the patchset, comment 2's
  confirm-diff would contain comment 1's approved work and the reviewer could not
  see what they were approving.
- **`/COMMIT_MSG` has no apply turn.** Commit-message comments ("add a `Bug:`
  trailer") are common and useful, but there is no file to `Edit` — they are
  routed into the finalize amend's message instead (§8). `/PATCHSET_LEVEL`
  comments are change-level: triage and reply only, never an edit.

### Reply prompt
- Offered for **every rejected comment**, not only the both-reject (★) cell. ★ is
  where Claude has the most ammunition (its own `disagree` justification), but a
  rejection against an `agree`/`unsure` verdict is precisely the case where the
  human held context Claude lacked — in the dry run the single most valuable
  rebuttal of six came from an `unsure` + human-reject. Scoping to ★ would never
  have drafted it. This also matches §6's fate table, which already said
  "REJECTED + reply (incl. ★ cell)".
- Respectful, concise, collegial, cites the specific reason, no condescension.
  Output `{reply}`.
- **Drafted and approved during Phase 2**, while the human is still at the
  keyboard — approval is a human step and Phase 3 is unattended. Where Claude's
  own justification does not apply (the human overrode an `agree` verdict), the
  draft is built from the human's stated reason for rejecting.

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

- **Resolve/reply timing**: only after a successful push. A resolution or "fixed"
  reply with no patchset behind it would be a lie.
- **Resolve and reply are ONE operation.** Gerrit has no mark-comment-resolved
  mutation — a thread is resolved by replying to it with `unresolved: false`. Both
  fates are the same POST to `/changes/{id}/revisions/current/review`, differing
  only in that boolean, so finalize issues **one** batched call. Two calls would
  model a mutation that does not exist and would open a partial-failure window
  where resolutions land and replies do not.
- **Pre-push revision check.** Re-GET the change immediately before pushing and
  abort unless `current_revision` still matches what was checked out. Human triage
  makes the window wide (tens of minutes), and the failure mode is not an error
  but a *wrong result*: an amend onto a stale revision pushes a patchset that
  silently reverts whatever the author uploaded meanwhile.
- **Staging is explicit.** Claude's `Edit` writes to the working tree and nothing
  runs `git add`, so `git commit --amend` over an unstaged tree amends only the
  message. Stage on each confirm. Prefer this over `-a` at amend time: it keeps
  "confirmed" and "staged" the same set, and `-a` would miss a newly created file.
- **`refs/for/<branch>` reads `<branch>` from `change.branch`** in the REST
  response. The worktree is on a detached HEAD from `refs/changes/...`; there is
  no local branch name to read.
- **`/COMMIT_MSG` routes to the amend's message**; `/PATCHSET_LEVEL` is reply-only.
  Neither is a file, so neither gets an apply turn (§7). The amend must therefore
  be able to rewrite the message — it cannot be a blanket `--amend --no-edit`.
- **Abort**: isolate, don't stash. Review happens in a dedicated worktree; abort
  leaves it in place (resumable / removable). The user's real tree is never
  mutated, so there is no WIP to protect.
- **Accumulation**: confirmed-but-unpushed edits pile up in the worktree between
  per-comment confirms; one amend + push at the end folds them together.
- **Auto mode**: dial, not switch (rung 0 manual / rung 1 semi / rung 2 full).
  v0 ships rung 0 only.
- **Apply granularity**: **per comment** (locked, §7) — the loop is uniformly
  per-comment. The **verdict** pass is the one exception: chunked per *file* (§7).

## 9. Verdicts rest on facts the code does not contain

Adjudicating a 40-comment change by hand surfaced that verdict quality is limited
less by code reading than by *out-of-code context* — CI config, tool versions,
team convention, roadmap, ticket numbers. The failure is invisible from the
adjudicator's side: a dependency can only be flagged `unsure` when it is
*noticed*, and when the missing context leaves no trace in the code, the code
looks complete and the verdict reads as confident.

Measured on that set: **12 verdicts rested on an external fact, and 9 of those had
been filed as confident** — 8 agrees and one *rejection*, which would have sent a
confidently wrong rebuttal to a reviewer who was right. A comment asking for a
`Bug:` trailer produced a fabricated ticket number, because nothing in the code
carries either the convention or the number.

**Hence required `depends_on`** (§7, `claude-driver`). Requiring the field with
`null` permitted makes "I had no way to know" a stated position rather than a
silent omission, and makes a fabricated value unreachable without first declaring
the gap. Two properties showed up in practice:

- **Dependencies aggregate.** The 12 collapsed into 10 distinct facts, one
  covering three verdicts — so triage shows a short go-find-out list, not 12
  separate shrugs.
- **Some are self-clearing.** Two commands settled one cluster, and all three
  verdicts held. The field separates doubt the tool can resolve itself from doubt
  that needs a human.

**Why this needs the driver.** Enforcement is the mechanism. `--json-schema` with
`depends_on` in `required` means the field structurally cannot be dropped; a
prompt merely *asking* for it has no validator behind it. This is the strongest
single argument for driving Claude as a subprocess rather than hosting the loop
inside a Claude Code session — see §10.

**Second-order.** Once a fact is answered it belongs in `openspec/config.yaml` or
`CLAUDE.md`. The verdicts that were *correct* for context reasons were correct
only because `config.yaml` recorded the style rules and this document recorded the
vybim-core deferral. Verdict quality tracks how much out-of-code context is
written down; the observed gaps clustered in tool versions and CI config, which
`openspec/` does not yet cover.

**Honest limit.** A forcing function, not a guarantee. It converts *noticed*
dependencies into visible ones. The 28 verdicts marked self-contained may still
hide unknown-unknowns, and there is no way to prove that residue is empty.

## 10. Architecture decision: Z3, the full TUI (RESOLVED)

This section previously held a GO/NO-GO: build the ratatui TUI, or drive the loop
natively inside a Claude Code session? The options were **Z0** (pure Claude Code +
a Gerrit MCP/skill), **Z1** (Z0 + a skill encoding the loop), **Z2** (a thin CLI
that sets up the worktree and launches `claude`), and **Z3** (the full TUI as
designed here). The standing lean was Z1-first.

**Decided: Z3.** A by-hand dry run of the whole loop against a simulated Gerrit —
itself effectively a Z1 run — settled it, and settled it the other way. Three
things it showed:

1. **Schema enforcement is not available to a skill.** The dry run's most
   consequential finding (§9) is that verdicts silently rest on out-of-code facts,
   and the fix is a *required* `depends_on`. That requirement is enforceable only
   where something validates the output — `--json-schema` on a subprocess call.
   Hosted inside a Claude Code session the verdict is the host's own reasoning;
   a skill can request the field in prose, but nothing rejects a response that
   drops it. The forcing function needs the driver.
2. **The loop's correctness lives in bookkeeping a conversation drifts on.**
   Snapshot before every apply turn (§7), diff and revert against that snapshot,
   key resolution off comment ids because line anchors go stale — the dry run
   watched all four of one file's anchors go stale after three edits. This is
   exactly the work code does reliably and a conversational session has to
   *remember* to do.
3. **The race window is a function of human triage time** (§8). Narrowing it wants
   single-keystroke triage, which is the TUI's whole point.

**Cost accepted, on the record.** `claude-driver` is coupled to `claude` CLI
surface that is not a stable API — the result-envelope shape, `structured_output`,
`--tools`, `--json-schema` inline, resume-across-permission-modes. All of it is
verified against **v2.1.220** and all of it can move under us. Z1 would pay none
of this. We are taking it because of point 1: the alternative is giving up the
only structural check we have on confident-but-groundless verdicts. Pin the
verified version, keep the driver behind its own trait (per `config.yaml`), and
treat a CLI upgrade as something to re-probe rather than assume.

**What this does not settle.** Whether conversational triage could have handled
the volume is now moot for v0, but the Gerrit layer stays the reusable core — it
was needed under every option and remains the first thing to build.

## 11. Open threads (next design tasks, not part of this change's build)

0. ~~Strategic GO/NO-GO (Z0–Z3).~~ **RESOLVED — Z3**, see §10.
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
