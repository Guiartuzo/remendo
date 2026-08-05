# Open decisions — `review-loop-foundation`

> **STATUS (2026-08-05): Tiers 1–4 are RESOLVED.** Tiers 1–3 were worked through
> in one session as intended (→ `design.md` §13); Tier 4 was settled while
> building `gerrit-client`, since a trait's error type is part of its surface
> (→ `design.md` §14). The items below are kept as the record of *what was
> undefined and why it mattered* — read §13 and §14 for what was chosen.
> **Tiers 5 and 6 remain open.**
>
> | # | Decision | Outcome |
> |---|---|---|
> | 1 | Gerrit connection and configuration | cwd inside a clone; base URL from `origin` (overridable); auth via `git credential fill`; TLS via system store → `http.sslCAInfo` |
> | 2 | Repository and worktree topology | worktree at `$XDG_STATE_HOME/remendo/<project>/<id>/`; relaunch **resumes**; verdicts cached on `(change, revision)` |
> | 3 | Thread vs. comment semantics | the **thread** is the unit; state = last comment's flag; whole exchange adjudicated; reply targets the last comment |
> | 4 | Comment provenance | exclude drafts; **include** own threads and robot comments; skip non-current-patchset threads and report the count |
> | 5 | Verdict schema shape | `{comment_id, verdict, justification, depends_on}`; `depends_on` a nullable **array**; **`confidence` dropped** |
> | 6 | Is `depends_on.verify` executed? | **No** — human-facing prose, never run |
> | 7 | Trait surfaces and their fakes | `GerritApi`/`GitCli`/`ClaudeDriver` are traits with named fakes; `similar` stays a module boundary |
> | 8 | Error model and partial failure | per-module `thiserror` enums; retry once, then an explicit **unadjudicated** state |
>
> Items 7 and 8 (Tier 4) were settled **2026-08-05** while building
> `gerrit-client` — see `design.md` §14. **Tier 5 and Tier 6 remain open.**
>
> One decision reversed a prior recommendation: item 4's suggestion to exclude
> self-authored comments and defer robot comments was **not** taken. Both are in
> scope, which grew the change — see §13 and `design.md` §12 item 9.

Things that are **undefined, not merely unbuilt**. Captured 2026-07-27, after the
dry run and the UI design pass, so they can be worked through in one focused
session rather than rediscovered one at a time mid-implementation.

Each item states what is undefined, why it matters, where it bites, the options,
and a recommendation where there is one. Items marked **OPEN** have no
recommendation — they are genuinely a call to make.

Tiers are by *consequence of guessing*, not by effort:

| Tier | Meaning |
|---|---|
| 1 | Blocks the first commit — cannot write the task without it |
| 2 | Guessing produces silently wrong behaviour, not an error |
| 3 | Blocks `claude-driver`; one has a security dimension |
| 4 | Required by `openspec/config.yaml`'s own rules, not yet done |
| 5 | Worth knowing before committing to the build |
| 6 | Safe to discover while building — listed so nothing is orphaned |

---

## Tier 1 — Blocks the first commit  ✅ RESOLVED (see `design.md` §13)

### 1. Gerrit connection and configuration

**Undefined.** How Remendo learns the Gerrit base URL and how it authenticates,
and where any of that is read from.

**Bites at** `tasks.md` 2.1/2.3, `design.md` §12 item 3. Nothing in
`gerrit-client` can be written first.

The auth mechanism is a known gap. The wider gap is that **there is no
configuration story at all** — not just the credential, but the base URL, and the
file or environment that supplies both.

- Auth: cookie / HTTP password (`Authorization: Basic`) / `.netrc`
- Source: config file (where? `$XDG_CONFIG_HOME/remendo/config.toml`?), env vars,
  git config keys, `.netrc` reuse
- Scope: global, or per-repository (a dev may hit more than one Gerrit)

**Recommendation.** HTTP password via `.netrc` — Gerrit generates them, `.netrc`
is where git already looks, and it dodges owning a secret store. Base URL from git
config on the clone (it is derivable from the `origin` remote in most setups) with
a config-file override. Verify against your actual Gerrit before committing.

### 2. Repository and worktree topology

**Undefined.** Which clone Remendo operates on, where the worktree is created, and
what happens when one already exists.

**Bites at** `tasks.md` 3.1/3.2, `specs/review-workspace/spec.md`. This is the
second thing you build.

`remendo <change-id>` is the whole interface, and a change id does not identify a
repository:

```
   remendo 12345
      │
      ├── which clone?  cwd assumed to be inside one?   ← never stated anywhere
      ├── the change carries a `project` field — validate it matches the clone?
      ├── where does the worktree live?
      │     .git/remendo/<id>/  ·  $XDG_STATE_HOME/remendo/<id>/  ·  sibling dir
      ├── naming: by change id? id+patchset? (re-review after a new ps)
      └── a worktree for this change ALREADY EXISTS → resume / recreate / error?
```

The last branch is guaranteed on the second test run: `review-workspace` says
abort leaves the worktree in place and reports where, so relaunching hits an
existing worktree, and nothing says what then.

**Recommendation.** cwd must be inside a clone; validate the change's `project`
against it and error clearly if it mismatches (`config.yaml` requires the
offending value in the message). Worktrees under `$XDG_STATE_HOME/remendo/`, keyed
by project + change id, so they survive `git clean` and do not pollute the repo.
An existing worktree is a **resume**, which also lands `design.md` §12 item 4
(resumable sessions) nearly for free.

**OPEN**: whether resume needs a state file to restore triage decisions, or just
re-fetches and re-runs the verdict pass. The latter is simpler and re-costs money.

---

## Tier 2 — Guessing produces silently wrong behaviour  ✅ RESOLVED (see `design.md` §13)

### 3. Thread semantics vs. comment semantics

**Undefined.** What "unresolved" means at thread level, and which comment id a
reply targets.

**Bites at** `specs/gerrit-client/spec.md` (fetch + filter, and the batched post),
`tasks.md` 2.4/2.6. This is the item most likely to ship a subtly broken tool.

Nothing in the specs distinguishes a comment from a thread:

```
  @rafa  "this is O(n²)"        unresolved: true
    └─ @you  "fixed in ps3"     unresolved: false     ← the thread is CLOSED

  Filtering on per-comment `unresolved: true` re-surfaces @rafa's comment
  on a thread that was already settled. Thread state is the LAST comment's
  flag, not any comment's.
```

Mirror problem on the write side: a reply needs `in_reply_to` set to the correct
comment in the thread. Neither the filter rule nor the reply target is specified.

`proposal.md` defers "comment threading beyond top-level unresolved comments" —
fine as *scope*, but identifying open threads correctly still requires walking the
thread, so the deferral does not remove the work.

**Recommendation.** Model a thread explicitly: group by `in_reply_to` chains, take
the last comment's `unresolved` as the thread's state, triage the thread's *first*
comment as the concern, and set `in_reply_to` to the thread's *last* comment id
when replying. Worth a fixture test against captured real responses.

### 4. Comment provenance filtering

**Undefined.** Whether Remendo triages your own comments, drafts, and robot
comments.

**Bites at** `tasks.md` 2.4.

Three separate include/exclude decisions, currently zero of them stated:

- **Your own comments.** Gerrit returns all authors. Adjudicating your own review
  notes is almost certainly wrong.
- **Drafts.** A separate endpoint (`/drafts`). Unpublished — almost certainly
  exclude.
- **Robot comments.** A third endpoint (`/robotcomments`) — CI linters. `design.md`
  §2 retired the robot-comment *framing* (humans write the comments we care
  about), but that says nothing about whether the endpoint gets fetched. Linter
  findings are arguably great apply-turn material.

**Recommendation.** Exclude self-authored and drafts in v0; state robot comments as
explicitly out of scope with a note that they are a natural v1 addition. The point
is to make all three choices *visible* rather than emergent.

---

## Tier 3 — Blocks `claude-driver`  ✅ RESOLVED (see `design.md` §13)

### 5. The verdict schema's concrete shape

**Undefined.** The actual JSON. `design.md` §7 names the fields
(`{comment_id, verdict, justification, confidence, depends_on}`) but two are
unspecified:

- **`confidence`** — float 0–1? An enum (`low`/`medium`/`high`)? It is rendered in
  the triage panel, so the UI needs to know.
- **`depends_on`** — `specs/claude-driver/spec.md` says nullable and singular
  ("naming any fact"). The dry-run notes describe it carrying `fact`, `kind`,
  `verify`, `flips_to`. So: a nullable object, or an array of them? And the dry run
  measured 12 verdicts collapsing to 10 distinct facts, so **aggregation happens
  somewhere** — in the schema, or in the app when it builds the triage view?

**Bites at** `tasks.md` 4.3, and `comment-triage`'s shared-dependency requirement
cannot be implemented without the answer.

**Recommendation.** `confidence` as an enum — a float invites false precision on a
judgment. `depends_on` as a nullable *array* of objects, aggregated in the app
layer by matching `fact`, keeping the schema per-verdict and the collapsing a UI
concern.

### 6. Is `depends_on.verify` machine-executed?  **← decide before writing the schema**

**Undefined**, and it cuts against a decision already made.

The dry run found some dependencies are self-clearing: "two commands settled that
cluster, and all three verdicts held." If `verify` is a command string Claude emits
and **Remendo runs it**, that reintroduces arbitrary command execution from model
output — immediately after `claude-driver` deliberately stripped shell access via
`--tools "Read,Edit"`.

```
  verify = advice rendered to the human   → safe; they run it, they judge it
  verify = command Remendo executes       → needs an allowlist, or a confirm
                                            gate, or it is a hole in the same
                                            wall §5 just built
```

**Bites at** the schema (item 5), and `specs/claude-driver/spec.md`'s scoping
requirement, which currently claims no shell access.

**Recommendation.** v0: `verify` is **human-facing text**, never executed. It keeps
the guarantee in `claude-driver` honest and costs only convenience. Revisit with an
explicit allowlist if self-clearing dependencies turn out to be frequent enough to
matter.

---

## Tier 4 — Required by `openspec/config.yaml`  ✅ RESOLVED (see `design.md` §14)

### 7. Trait surfaces and their fakes

**Undefined.** `config.yaml`'s design rule: *"State which third-party lib each new
trait wraps, and the fake used to test it."* Only the driver trait is named
anywhere (`design.md` §10, `tasks.md` 4.1/4.2).

Missing, though all are mandated by the dependency rules:

| Trait | Wraps | Fake |
|---|---|---|
| Gerrit | `ureq` (blocking HTTP) | — undefined |
| Git | the `git` CLI | — undefined |
| Claude driver | the `claude` CLI | named, surface undefined |
| Diff | `similar` | — undefined |

**RESOLVED 2026-08-05 — the full surfaces are in `design.md` §14.** Three of the
four are traits (`GerritApi`/`ureq`, `GitCli`/the git CLI, `ClaudeDriver`/the
`claude` CLI), each with a named fake. The fourth is **not** a trait: `similar`
is a pure function with no I/O to stub, so `config.yaml`'s wrap requirement is
met by confining it to the `diff_view` module, and a trait would be indirection
with a single implementation.

Two shapes fell out of decisions rather than convenience: credentials sit on
`GitCli` (not `GerritApi`) because they come from `git credential fill`, and
`GerritApi` has **no `drafts` method at all** — excluding drafts was a decision,
and a trait without the method cannot be talked into fetching them later.

These method surfaces are the app's actual internal API. Worth designing on
purpose rather than accreting them one call site at a time.

### 8. Error model and partial-failure policy

**Undefined.** There is no error taxonomy, though `config.yaml` makes error
quality a first-class rule (messages must carry the offending value and the
expected shape).

The interesting case is created by per-file verdict chunking (finding #13):

```
  verdict pass over 7 files → file 3 fails (timeout / budget / API error)
     proceed with 6 files' verdicts?   ← partial triage, silently incomplete
     retry file 3?
     abort the whole run?
```

Others needing a defined behaviour: `claude` absent from `PATH`;
`--max-budget-usd` ceiling hit mid-run (which turn? what state is kept?); network
loss during triage; worktree checkout conflict; a change with **zero** unresolved
comments (should exit gracefully, currently unstated).

**Recommendation.** Retry once, then surface the failed file as an explicit
"unadjudicated" state in the tree rather than silently proceeding — a missing
verdict must never look like an empty one, which is the same failure mode
finding #14 is about.

**RESOLVED 2026-08-05 — recommendation adopted; see `design.md` §14.** Errors are
per-module `thiserror` enums (`GerritError`, `GitError`, `DriverError`) composed
upward with `#[from]`, rather than one crate-wide enum that would force every
call site to match variants its layer cannot produce. Every variant carries the
offending value. The two cases already built show what that buys: a missing XSSI
guard reports an HTML login page rather than "expected value at line 1 column 1",
and a TLS failure names the host and points at `git config --get
http.sslCAInfo`. Zero-unresolved-threads exits gracefully; `claude` missing from
`PATH` is reported at startup by name.

---

## Tier 5 — Worth knowing before committing to the build

### 9. Cost per change

**Unmeasured.** The project exists because review volume exploded, and nobody has
priced a run.

Datum: one *trivial* schema-constrained probe (`claude` 2.1.220, verdict schema,
one sentence in) cost **$0.14** with 13.6k cache-creation tokens. A 40-comment
change is roughly 7 verdict turns + ~25 apply turns ≈ 32 spawns, each carrying
real file context. The order of magnitude is plausibly dollars-per-change, not
cents — and multiplied by team volume that is a number worth having *before*
Phase 3 works, not after.

Fittingly this is itself a textbook `depends_on` fact: absent from the code,
invisible unless noticed, and settleable with one measured run.

**Recommendation.** Instrument from day one — the result envelope already returns
`total_cost_usd` per turn (verified), so accumulate it per change and display it.
Then one real run answers this permanently.

---

## Tier 6 — Safe to discover while building

Listed only so they are not mistaken for oversights. All reversible, all better
decided with something running. Detail lives in `design.md` §12.

- Phase 1 → Phase 2 panel geometry (§12 item 5) — the tree added a third column,
  which tightens this.
- `n` semantics with deferrals in play (§12 item 6).
- Whether the tree filters the triage queue or jumps into a global one (§12 item 7).
- Reply drafts for hand-fixed comments (§12 item 8).
- Prompt prose tuning (§12 item 2).
- Keybinding assignments and pane ratios.

---

## Verifications owed (not decisions)

From `dry-run-findings.md`'s "Not verified" section — these need a real Gerrit and
a real compile, not a discussion:

- **`unresolved: false` actually resolves threads on your Gerrit version**
  (`tasks.md` 8.4). The mechanism was exercised only against a local stub.
  **Still owed** — needs a real Gerrit, so it cannot be closed before §7.
- ~~**Nothing has been compiled.**~~ **CLEARED 2026-08-05.** The dry-run box had
  no `cargo`; this one does (1.96.0). The crate builds and `cargo test` runs in
  CI and locally, so code is type-checked rather than reviewed logically.
- **`--tools` vs `acceptEdits` orthogonality** — the probe showed `--tools`
  constraining alongside `acceptEdits`, not that omitting it would grant Bash.
  **Still owed**, and now compounded by the version drift below.
- **NEW: the pinned `claude` version is stale.** `config.yaml` and `tasks.md`
  4.11 pin **2.1.220**; the development box runs **2.1.222**. Everything about
  the envelope shape, `structured_output`, `--tools`, inline `--json-schema` and
  resume-across-permission-modes was probed against 2.1.220 only.

  **DEFERRED to the start of §4 (decided 2026-08-05).** Re-probing costs real
  money — the dry run measured $0.14 for one trivial schema probe — and it
  blocks nothing until `claude-driver` is built. Deferring also avoids paying
  twice if the box upgrades again first. The obligation does not lapse: **§4
  must not begin until the pin is re-verified or the box is pinned back**, and
  task 4.11 is the gate.
- **NEW: the release job has never run.** `tasks.md` 1.8 is implemented but no
  tag has been pushed, so the `objdump` glibc-2.31 guard is untested. It also
  only covers Remendo's own binary — `tasks.md` 8.6 (whether `claude` and git
  2.25.1 run on Ubuntu 20.04) remains entirely unverified.
