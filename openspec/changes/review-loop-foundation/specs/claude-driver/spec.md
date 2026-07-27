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

### Requirement: Apply turns are scoped
The application SHALL constrain apply turns to editing tools only (no shell
access) and SHALL limit their filesystem reach to the directory of the file being
edited, and MAY set a spending ceiling per invocation.

#### Scenario: Apply turn is limited to editing tools and the file's directory
- **WHEN** an apply turn is invoked for an accepted comment
- **THEN** it is granted only read/edit tooling and filesystem access scoped to
  that comment's file's directory

### Requirement: Structured verdict output
The application SHALL request verdicts as schema-validated structured output, each
verdict carrying at least an adjudication of `agree`, `disagree`, or `unsure` and
a justification, so the UI consumes structured data rather than parsing prose.

#### Scenario: A verdict is returned as structured data
- **WHEN** the verdict turn adjudicates a comment
- **THEN** it returns a structured result containing an `agree`/`disagree`/
  `unsure` value and a justification
