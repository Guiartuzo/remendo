# Dry-run findings — `review-loop-foundation`

> **STATUS: all 14 findings folded into the spec** (2026-07-27). This document is
> kept as the *evidence record* — the reproductions, probes and measurements
> behind the corrections. It is no longer a to-do list; the specs are the source
> of truth. Where a finding's reasoning is load-bearing it now also lives in
> `design.md` (§5 flag corrections, §7 prompt contract, §8 settled decisions,
> §9 out-of-code dependencies).
>
> The dry run also resolved the strategic GO/NO-GO that stood at the top of
> `design.md`'s open threads: **Z3**, the full TUI — see `design.md` §10. The
> pivotal argument came from finding #14: a *required* `depends_on` is only
> enforceable where something validates the output, which needs the subprocess
> driver.
>
> Two items remain **open** rather than fixed, both from "Not verified" below:
> Gerrit auth (`tasks.md` 2.3) and confirming the `unresolved:false` resolve
> mechanism against a real Gerrit (`tasks.md` 8.4). Both are carried forward in
> `open-decisions.md`, which also catalogues the pre-build decisions a later
> design pass surfaced — several of them descendants of findings here (the
> `depends_on` schema shape, and whether its `verify` field is machine-executed).

Findings from reviewing the `review-loop-foundation` change and then **executing its
loop by hand** against a simulated Gerrit, before any Rust exists.

Three sources, marked per finding:

- **spec review** — read from `proposal.md` / `design.md` / `specs/`.
- **dry run** — surfaced by running the loop end to end (fetch → worktree → verdict →
  triage → apply → confirm → amend → push → resolve/reply) against a stub Gerrit
  serving real HTTP with the `)]}'` guard.
- **probe** — verified empirically against `claude` **2.1.220**, the version
  `proposal.md:98` pins.

Everything below is reproducible; nothing is inferred from documentation alone unless
the finding says so.

---

## Severity index

| # | Finding | Where | Source |
|---|---|---|---|
| 1 | Reject reverts to the patchset baseline and destroys earlier confirmed edits | `specs/fix-application/spec.md:35` | spec review + **dry run** |
| 2 | `--output-format json` returns an envelope, so the verdict parse can never work | `tasks.md:37` | **probe** |
| 3 | `--add-dir` widens filesystem reach; it does not confine | `specs/claude-driver/spec.md:35` | spec review + **probe** |
| 4 | No pre-push revision check — a concurrent patchset gets silently reverted | `specs/change-submission/spec.md` | spec review |
| 5 | Gerrit has no resolve mutation; resolve *is* reply-with-`unresolved:false` | `tasks.md:72` | **dry run** |
| 6 | Confirm-diff baseline is wrong for the 2nd+ comment in a file | `specs/fix-application/spec.md:25` | **dry run** |
| 7 | Nothing is ever staged, but three places say "staged" | `tasks.md:69` | spec review |
| 8 | `/COMMIT_MSG` and `/PATCHSET_LEVEL` comments are not files | `design.md:89` | **dry run** |
| 9 | Comment line anchors drift as earlier edits land | `design.md:253` | **dry run** |
| 10 | Reply drafting/approval has no phase in the flow | `design.md:95-101` | spec review |
| 11 | `refs/for/<branch>` — `<branch>` has no stated source | `design.md:97` | spec review |
| 12 | Reply scoped to the ★ cell only is too narrow | `design.md:262` | **dry run** |
| 13 | Verdict pass is the one non-per-comment step | `specs/comment-triage/spec.md:3` | spec review |
| 14 | Verdicts silently depend on facts absent from the code | — | **dry run** |

---

## 1. Reject destroys earlier confirmed edits

**Severity: highest — silent data loss.**

`specs/fix-application/spec.md:35` requires reject to "restore the affected file to its
**patchset-baseline** contents", and `tasks.md:62` implements that as
`git checkout -- <file>`. But `specs/claude-driver/spec.md:15` and `design.md:147`
require edits to **compose** — each apply turn reads what the previous turn wrote. Those
two requirements contradict each other, and the spec picks the destructive reading.

Observed. Two comments on `src/gerrit/response.rs`: confirm comment 1 (`strip_prefix`
rewrite), apply comment 2, reject it.

```
before reject: strip_prefix 2 hits   (comment 1 confirmed)
after  reject: strip_prefix 0 hits   <-- comment 1's confirmed work is gone
```

**Fix.** Snapshot the file immediately *before* each apply turn; on reject, restore that
snapshot. "Patchset baseline" is only correct for the first comment in a given file. The
phrase appears at `specs/fix-application/spec.md:25`, `:35`, `:42`, `design.md:90`,
`:257`, `proposal.md:50` and `tasks.md:60` — it is wrong in all of them.

## 2. The verdict parse can never succeed

`tasks.md:37` parses `--output-format json` directly into the verdict payload. It is an
envelope, not the payload:

```
$ claude -p 'reply with exactly: ok' --output-format json
{"is_error":false,"duration_api_ms":4671,"num_turns":1,"stop_reason":"end_turn",
 "session_id":"15b4237d-...","total_cost_usd":0.1128,"usage":{...},...}
```

**Fix.** Deserialize the envelope, check `is_error`, then parse the verdict payload out
of its `result` field. Worth a fake in tests, per `openspec/config.yaml`'s
mock-external-IO rule.

> **Correction (re-probed 2026-07-27, v2.1.220).** The probe above omitted
> `--json-schema`, and with the schema attached the envelope carries a
> **`structured_output`** field holding the already-parsed object. Read that, not
> `result` — `result` is the same JSON as a string and needs a second decode. Two
> further facts from the re-probe: `--json-schema` takes the schema **inline**, not
> a file path (a path fails with `--json-schema is not valid JSON`), and the
> structured-output path returns `stop_reason: "tool_use"` with `num_turns: 2`, so
> gate on `is_error` and never on `stop_reason == "end_turn"`.

## 3. `--add-dir` does not confine

`specs/claude-driver/spec.md:35` says apply turns "SHALL limit their filesystem reach to
the directory of the file being edited", and `design.md:157` calls this a structural
guard. `--add-dir` is *additional* directories — it widens the allowed set on top of cwd.

Probed: cwd at the worktree root, `--add-dir <wt>/src/a`, asked to read `src/b/g.rs`:

```
result: SECRET_MARKER = 8675309  (src/b/g.rs:1)
is_error: False
```

It read a file in neither directory, because cwd stays fully readable. Passing
`--add-dir` grants strictly *more* reach than omitting it.

**Fix.** Drop the confinement claim. The honest guards are `--tools "Read,Edit"`
(no shell) plus the confirm-diff. Restate the requirement as what is actually enforced —
`design.md:171`'s parenthetical "still constrained by `--tools`/`--add-dir`" needs the
same correction.

## 4. Concurrent patchset race

Nothing re-checks the change before pushing. Fetch ps3, triage for twenty minutes, the
author uploads ps4 meanwhile, your amend is based on ps3 — the push lands a ps5 that
silently reverts their work. Human triage makes the window wide, and it fails as a wrong
result rather than an error.

**Fix.** Re-fetch immediately before push and abort unless `current_revision` still
matches what was checked out. Add it as a scenario in `change-submission`. Verified
working in the dry run:

```
race check OK: still on 944040b
```

## 5. Resolve and reply are one operation, not two

`tasks.md:72` and `proposal.md:75` describe "batch-resolve accepted comments;
batch-post approved replies" as two batched calls. Gerrit has **no** mark-comment-resolved
mutation. A thread is resolved by *replying* to it with `unresolved: false`.

So both fates are the same POST to `/changes/{id}/revisions/current/review`, differing
only in a boolean. One call closed the whole 6-comment review in the dry run:

```
RESOLVED   /COMMIT_MSG:7              (reply to c6a1b2c3)
RESOLVED   src/gerrit/response.rs:20  (reply to c1a1b2c3)
STAYS OPEN src/gerrit/response.rs:18  (reply to c4a1b2c3)
```

**Fix.** Collapse the two requirements in `change-submission` into one batched call. This
also removes a partial-failure window where resolves land and replies do not.

## 6. Confirm-diff shows already-confirmed work

Same root cause as #1. `specs/fix-application/spec.md:25` diffs against the patchset
baseline, so once comment 1 is confirmed, comment 2's confirm-diff contains **both**
edits and the reviewer cannot see what they are approving. Observed: comment 2's diff
included comment 1's entire `strip_xssi` rewrite.

**Fix.** Diff against the pre-turn snapshot from #1.

## 7. Nothing is ever staged

`tasks.md:69` amends "all staged edits"; `specs/review-workspace/spec.md:37` and `:44`
promise "staged-but-unpushed edits". Claude's `Edit` writes to the working tree and
nothing runs `git add`, so `--amend` with nothing staged amends only the message.

**Fix.** `git add` on confirm (or `-a` at amend time), and change "staged" to "modified"
in the three places that mean the latter.

## 8. `/COMMIT_MSG` and `/PATCHSET_LEVEL` are not files

Gerrit anchors commit-message comments on the pseudo-path `/COMMIT_MSG` and change-level
comments on `/PATCHSET_LEVEL`. The apply flow computes "the file's directory"
(`design.md:89`) for a path that does not exist on disk, and there is nothing to `Edit`.

Commit-message comments are common ("add a `Bug:` trailer") and genuinely useful — they
fold naturally into the finalize amend. Confirmed in the dry run by applying one.

**Fix.** Route `/COMMIT_MSG` comments to the amend's message rather than a file edit;
surface `/PATCHSET_LEVEL` for reply only.

> **Two corrections.** (a) The note here said this "collides with #9" over
> `--amend --no-edit`; `--no-edit` appears nowhere in `openspec/`, so there was
> nothing to collide with — the real requirement is simply that the amend must be
> *able* to rewrite the message, now stated in `change-submission`. (b) Missing
> from the finding: `/COMMIT_MSG` line numbers address Gerrit's synthetic
> rendering, which prepends `Parent`/`Author`/`AuthorDate`/`Commit`/`CommitDate`
> and a blank line before the subject. Mapping a comment's line to the real
> message must subtract that offset (`tasks.md` 2.5).

## 9. Line anchors drift

Comments are anchored at `(file, line)` against the patchset, and the apply prompt
(`design.md:253`) identifies the comment by that line. After three edits to one file
during the dry run, every anchor (18, 20, 29, 32) was stale.

**Fix.** Feed the quoted code from the comment's context and have the turn re-read the
file; do not rely on the line number. Resolution is by comment id, so finalize is safe.

## 10. Reply approval has no place in the flow

`design.md:95-101` runs Phase 3 as amend → push → resolve → reply, but the human must
**approve** each draft (`specs/change-submission/spec.md:28`) and no phase contains that
step.

**Fix.** Draft and approve at the end of Phase 2, so Phase 3 is fully unattended.

## 11. `<branch>` has no stated source

`design.md:97`, `tasks.md:70` and `specs/gerrit-client/spec.md:50` all push to
`refs/for/<branch>`, but the worktree is on a detached HEAD from `refs/changes/...` —
there is no local branch to read.

**Fix.** State that it comes from `change.branch` in the REST response. Confirmed in the
dry run (`target branch: main`).

## 12. ★-only reply scoping is too narrow

`design.md:262` scopes reply drafting to the both-reject (★) cell. In the dry run the most
valuable rebuttal of six came from an `unsure` + human-reject: it cited `design.md`'s own
deferral of the vybim-core extraction back at the reviewer. Under the current scope that
reply is never drafted.

**Fix.** Offer a draft for **every** rejection, not just ★.

## 13. The verdict pass is the one non-per-comment step

`design.md:266` locks apply to per-comment for clean 1:1 attribution, but
`specs/comment-triage/spec.md:3` runs a single verdict turn over *all* comments. On a
40-comment change that is one large turn returning 40 schema-validated verdicts after
reading five files — where truncation and blanket "looks fine" verdicts appear.

**Fix.** Chunk by file. Keeps the shared-context benefit and bounds the payload.

## 14. Verdicts silently rest on facts absent from the code

**The most consequential finding, and the hardest to fix.**

Adjudicating 40 comments surfaced that verdict quality is limited less by code reading
than by *out-of-code context* — CI config, tool versions, team convention, roadmap. Worse,
the failure is invisible from the adjudicator's side: a dependency can only be flagged
`unsure` when it is *noticed*, and when the missing context leaves no trace in the code,
the code looks complete and the verdict reads as confident.

Measured on the 40-comment set: **12 verdicts rested on an external fact, and 9 of those
had been filed as confident** — 8 agrees and one *rejection*, which would have sent a
confidently wrong rebuttal to a reviewer who was right.

Concrete instance from this dry run: a comment asked for a `Bug:` trailer. Nothing in the
code carries the trailer convention or the ticket number, so the applied fix invented one:

```
Bug: 41827      <-- fabricated
```

**Fix — add a required `depends_on` to the verdict schema.** Requiring the field (with
`null` permitted) makes "I had no way to know" a stated position instead of a silent
omission, and makes a fabricated value impossible to reach without declaring the gap.
Schema drafted at `skill/verdict-schema.json` in the scratch lab; carries `fact`, `kind`,
`verify`, `flips_to`.

Two properties showed up in practice:

- **Dependencies aggregate.** The 12 collapsed into 10 distinct facts, one covering three
  verdicts. The verdict pass can hand back a short go-find-out list rather than 12
  separate shrugs.
- **Some are self-clearing.** Two commands settled that cluster (findings #2 and #3
  above), and all three verdicts held. The field distinguishes doubt the tool can resolve
  itself from doubt that needs a human.

**Second-order fix.** Once a fact is answered it belongs in
`openspec/config.yaml` / `CLAUDE.md`. The verdicts that were *correct* for context reasons
were correct only because `config.yaml` recorded the style rules and `design.md` recorded
the deferral. Verdict quality tracks how much out-of-code context is written down; the
gaps clustered in tool versions and CI config, which `openspec/` does not yet cover.

**Honest limit.** This is a forcing function, not a guarantee. It converts *noticed*
dependencies into visible ones. The 28 verdicts marked self-contained may still hide
unknown-unknowns, and there is no way to prove that residue is empty.

---

## Not verified

- **Nothing was compiled.** No `cargo`/`rustc` on the dry-run box, so the three applied
  edits are logically reviewed but not type-checked. A real run should end with
  `cargo test` in the worktree before the amend.
- **Real Gerrit.** All REST traffic went to a local stub. The XSSI guard, comment
  filtering and one-batch finalize are exercised; **auth is not** (`design.md:321` still
  lists it as an open thread), and finding #5 should be confirmed against your Gerrit
  version.
- **`--tools` vs `acceptEdits` orthogonality** (part of #3) is supported but not fully
  isolated: the probe shows `--tools` constraining alongside `acceptEdits`, not that
  omitting `--tools` would grant Bash.
- **Environment note.** The dry-run box has git 2.25.1, which lacks `git init -b`.
  `git worktree add --detach` works fine; worth knowing if setup gets scripted.

## Suggested spec edits

1. `fix-application` — snapshot-per-turn replaces patchset-baseline for both the
   confirm-diff and the revert. *(#1, #6)*
2. `change-submission` — one batched review POST; add the pre-push revision check.
   *(#4, #5)*
3. `claude-driver` — correct the `--add-dir` requirement to what is enforced; add
   envelope parsing; add required `depends_on` to the verdict schema. *(#2, #3, #14)*
4. `review-workspace` / `tasks.md` — "modified", not "staged"; add `git add` on confirm.
   *(#7)*
5. `comment-triage` — chunk the verdict pass by file. *(#13)*
6. `design.md` — place reply drafting in Phase 2; state `<branch>` comes from
   `change.branch`; define `/COMMIT_MSG` and `/PATCHSET_LEVEL` handling; anchor apply
   prompts on quoted code rather than line numbers; widen reply scope past ★. *(#8–#12)*
