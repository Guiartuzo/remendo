## ADDED Requirements

### Requirement: One resume-based Claude session per change
The application SHALL drive Claude Code as a single logical session per change,
identified by a generated session id, invoked as a sequence of non-interactive
one-shot calls that resume that session. Each call SHALL run as a
spawn-wait-parse subprocess; the application SHALL NOT require a long-lived
bidirectional Claude process in v0.

#### Scenario: Turns share context via the session id
- **WHEN** the application issues a later Claude turn for a change
- **THEN** it resumes the same session id so the turn has the context of prior
  turns for that change

### Requirement: Edits compose through the working tree
Because successive apply turns operate on the same worktree, each apply turn SHALL
observe the file contents written by prior confirmed turns, so that edits from
multiple comments on the same file compose rather than contradict.

#### Scenario: A later edit sees an earlier edit
- **WHEN** an apply turn edits a file that a prior confirmed turn already changed
- **THEN** the later turn operates on the already-changed contents

### Requirement: Verdict turns cannot modify files
The application SHALL run the comment-adjudication (verdict) turn in a permission
mode that structurally prevents file modification (`plan`), so the verdict pass
cannot edit the worktree.

#### Scenario: Verdict pass leaves the worktree unchanged
- **WHEN** the verdict turn runs over a change's comments
- **THEN** no file in the worktree is modified by that turn

### Requirement: Apply turns are restricted to editing tools
The application SHALL constrain apply turns to read and edit tooling only, with no
shell access, and MAY set a spending ceiling per invocation. The apply turn's
working directory SHALL be the worktree root, so the turn can re-read the file it
is editing and its neighbours.

This requirement SHALL NOT be stated as filesystem confinement. `--add-dir` grants
*additional* directories on top of an always-readable working directory; passing
it widens reach rather than narrowing it, so it cannot scope a turn to one
directory. The enforced blast-radius guards are the tool restriction (no shell)
and the per-turn confirm-diff, which is what the user actually approves.

#### Scenario: Apply turn has no shell access
- **WHEN** an apply turn is invoked for an accepted comment
- **THEN** it is granted read and edit tooling only, and cannot execute shell
  commands

#### Scenario: An over-broad edit is caught by the confirm-diff
- **WHEN** an apply turn edits more than the comment's concern required
- **THEN** the excess appears in that turn's confirm-diff and the user can reject
  it, restoring the pre-turn snapshot

### Requirement: Structured verdict output
The application SHALL request verdicts as schema-validated structured output, each
verdict carrying at least an adjudication of `agree`, `disagree`, or `unsure`, a
justification, and a `depends_on` field (see below), so the UI consumes structured
data rather than parsing prose.

#### Scenario: A verdict is returned as structured data
- **WHEN** the verdict turn adjudicates a comment
- **THEN** it returns a structured result containing an `agree`/`disagree`/
  `unsure` value and a justification

### Requirement: Verdicts declare their out-of-code dependencies
The verdict schema SHALL carry a **required** `depends_on` field, nullable but not
omittable, naming any fact outside the code that the verdict rests on — CI
configuration, tool versions, team convention, roadmap, ticket identifiers. A
verdict that genuinely needs nothing beyond the code SHALL state `null`.

Requiring the field makes "I had no way to know this" a declared position rather
than a silent omission, and makes fabricating an unknown value (an invented ticket
number, an assumed convention) unreachable without first declaring the gap. This
is a forcing function over *noticed* dependencies, not a guarantee against
unknown-unknowns.

#### Scenario: A context-dependent verdict declares the fact it rests on
- **WHEN** the verdict turn adjudicates a comment whose correctness depends on a
  fact not present in the code
- **THEN** the returned verdict names that fact in `depends_on` rather than
  presenting itself as self-contained

#### Scenario: A self-contained verdict declares no dependency
- **WHEN** the verdict turn adjudicates a comment decidable from the code alone
- **THEN** the returned verdict carries `depends_on: null`

#### Scenario: The field cannot be skipped
- **WHEN** the verdict turn returns a result omitting `depends_on`
- **THEN** the result fails schema validation and is not presented as a verdict

### Requirement: Claude CLI output is parsed as an envelope
The application SHALL deserialize the Claude CLI's JSON output as a result
envelope, check its `is_error` field, and read the schema-conformant object from
the envelope's structured-output field — rather than treating the envelope as the
verdict payload or re-parsing the human-readable `result` string. `--output-format
json` returns that envelope, not the requested payload, so parsing its output
directly into a verdict cannot succeed. Envelope handling SHALL be exercised in
tests through a named fake implementing the driver trait.

#### Scenario: A successful turn yields its payload from the envelope
- **WHEN** a structured-output turn completes successfully
- **THEN** the envelope is deserialized, `is_error` is false, and the verdict
  payload is read from the envelope's structured-output field

#### Scenario: A failed turn is surfaced, not parsed
- **WHEN** a turn's envelope reports `is_error` true
- **THEN** the failure is surfaced to the user and no verdict payload is
  fabricated from the envelope
