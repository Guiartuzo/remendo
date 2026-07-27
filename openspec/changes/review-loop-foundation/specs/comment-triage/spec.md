## ADDED Requirements

### Requirement: Verdict pass over all unresolved comments, chunked by file
On loading a change, the application SHALL run a Claude verdict pass that
adjudicates every unresolved comment, producing for each a verdict
(`agree`/`disagree`/`unsure`) and a justification, before or as the user begins
triage.

The pass SHALL be issued as one turn **per file**, each turn covering that file's
comments, rather than a single turn over every comment in the change. A
comment-dense change would otherwise put dozens of adjudications in one response,
where truncation and undifferentiated blanket verdicts appear. Chunking keeps each
turn's payload bounded while the shared session retains context across chunks.

#### Scenario: Every unresolved comment receives a verdict
- **WHEN** a change with unresolved comments is loaded
- **THEN** each unresolved comment has an associated verdict and justification
  available to the triage UI

#### Scenario: Comments are adjudicated in per-file turns
- **WHEN** the verdict pass runs over a change whose comments span several files
- **THEN** one verdict turn is issued per file, each covering that file's comments

### Requirement: Declared verdict dependencies are surfaced during triage
The triage UI SHALL display a verdict's `depends_on` value alongside its
justification, so the human sees which verdicts rest on facts outside the code
before deciding. Where several verdicts declare the same fact, the application
SHALL present it once as a shared dependency rather than repeating it per comment.

#### Scenario: A declared dependency is visible before the decision
- **WHEN** the user triages a comment whose verdict declares a `depends_on` fact
- **THEN** that fact is shown with the verdict, so the user can weigh the verdict
  as conditional rather than as self-contained

#### Scenario: A shared dependency is presented once
- **WHEN** several verdicts declare the same out-of-code fact
- **THEN** it is surfaced once as a dependency covering those verdicts

### Requirement: Replies are drafted and approved during triage
The application SHALL offer a Claude-drafted reply for every comment the user
rejects — not only those Claude also judged `disagree` — and SHALL capture the
user's approval, edit, or refusal of that draft **during the triage phase**,
before finalize begins. Finalize is therefore unattended: it posts only drafts
already approved.

A rejection where Claude judged `agree` or `unsure` is a case where the human
holds context Claude lacked, which is often exactly where a rebuttal is most worth
sending; scoping drafts to the both-reject case alone would never draft it.

#### Scenario: Every rejection is offered a draft
- **WHEN** the user rejects a comment during triage
- **THEN** a reply draft is offered for it regardless of what Claude's verdict for
  that comment was

#### Scenario: Approval is captured before finalize
- **WHEN** the user finishes triage
- **THEN** each rejected comment's reply is already approved, edited-and-approved,
  or declined, and finalize requires no further user input for replies

#### Scenario: A declined draft posts nothing
- **WHEN** the user declines a drafted reply during triage
- **THEN** no reply is posted for that comment at finalize

### Requirement: Triage view shows the document, the comment, and the verdict
The application SHALL present triage as a document pane showing the commented
document with the comment's anchored line or range **highlighted in place**,
carrying the reviewer comment and its **author name**, alongside a pane showing
Claude's verdict and justification for that comment. The author name is shown for
the human's judgment only; it SHALL NOT be provided to Claude for the verdict (the
verdict is judged on technical merit).

The document pane SHALL render the document the comment is anchored to, which is
the source file for a file-anchored comment, the commit message for a
`/COMMIT_MSG` comment, and a change-overview document for a `/PATCHSET_LEVEL`
comment.

#### Scenario: Document, comment, author, and verdict are shown together
- **WHEN** the user is triaging a comment
- **THEN** the commented document is shown with the comment's line or range
  highlighted, together with the comment prose and its author name, alongside
  Claude's verdict and justification for the same comment

#### Scenario: A commit-message comment is shown against the message
- **WHEN** the user triages a comment anchored on `/COMMIT_MSG`
- **THEN** the document pane shows the commit message with the commented line
  highlighted

#### Scenario: Verdict is not influenced by the author
- **WHEN** the verdict pass adjudicates a comment
- **THEN** the comment's author identity is not part of the input that produces
  the verdict

### Requirement: The document pane is read-only and syntax highlighted
The application SHALL render source in the document pane with syntax highlighting,
and SHALL NOT provide in-application code editing in v0. Hand-written fixes are
expected to happen in the user's own editor against the worktree, so the
application SHALL report the worktree path in a form the user can act on.

Editing a *comment's prose* before it feeds an apply turn is a separate capability
and is unaffected by this requirement.

#### Scenario: Source is highlighted but not editable
- **WHEN** the user views a source file in the document pane
- **THEN** it is syntax highlighted, and no keystroke modifies its contents

### Requirement: Externally modified files are re-read
The application SHALL detect that a file in the worktree changed on disk and
re-read it, so that fixes made in an external editor are reflected in the
document pane and reach the finalize amend. Because the document pane is
read-only there is no in-application buffer to reconcile, so a reload SHALL NOT
prompt the user to resolve a conflict.

If a file changes on disk while a confirm-diff for that file is awaiting the
user's decision, the application SHALL invalidate that pending confirm-diff rather
than let the user confirm a diff computed against contents that no longer match
the file.

#### Scenario: An external fix is picked up
- **WHEN** the user edits a worktree file in an external editor and returns to the
  application
- **THEN** the file is re-read and the document pane reflects the new contents

#### Scenario: A pending confirm-diff is invalidated by an external edit
- **WHEN** a file changes on disk while a confirm-diff for that file is awaiting
  confirmation
- **THEN** that confirm-diff is invalidated and cannot be confirmed as shown

### Requirement: File tree navigation over the change
The application SHALL provide a file tree, toggleable with a keystroke, listing
the change's files annotated with each file's comment count and triage progress,
so the user can choose which file to review first. The tree SHALL include
`/COMMIT_MSG` and `/PATCHSET_LEVEL` as entries distinguishable from real files
whenever the change carries comments anchored on them.

#### Scenario: Tree shows where attention is still needed
- **WHEN** the user opens the file tree partway through triage
- **THEN** each file is listed with its comment count and how many of those
  comments have been decided

#### Scenario: Pseudo-path comments are reachable from the tree
- **WHEN** the change has a comment anchored on `/COMMIT_MSG` or
  `/PATCHSET_LEVEL`
- **THEN** the tree offers a corresponding entry, marked as not being a file

#### Scenario: Tree is dismissable
- **WHEN** the user toggles the tree off
- **THEN** it is hidden and its space is returned to the rest of the view

### Requirement: Manual accept, reject, defer, and edit
The application SHALL let the user, per comment, accept it, reject it, defer it to
be revisited later, mark it as already fixed by hand, or edit its prose before it
is used, and SHALL navigate between comments. v0 SHALL provide manual triage only,
with no automatic acceptance or rejection.

Because deferral makes the queue non-linear, the application SHALL distinguish
moving to the adjacent comment from moving to the next comment with no decision
yet.

#### Scenario: User decides each comment
- **WHEN** the user acts on a comment during triage
- **THEN** the comment is recorded as accepted, rejected, deferred, fixed by hand,
  or edited according to the user's choice

#### Scenario: Deferred comments are revisitable
- **WHEN** the user defers a comment and later asks for the next undecided comment
- **THEN** the deferred comment is offered again, without re-walking comments that
  already have a decision

### Requirement: A comment can be marked as fixed by hand
The application SHALL let the user mark a comment as already fixed by hand,
recording a fate that resolves the comment without issuing an apply turn and
without presenting a confirm-diff. Since v0 has no in-application editor, such a
fix is made in the user's own editor against the worktree, so the application
SHALL re-read the affected file so the fix reaches the finalize amend.

None of the other fates fits this case: rejecting would post a rebuttal to a
reviewer the user agreed with, deferring would leave a fixed comment unresolved,
and accepting would spend an apply turn redoing work already done.

#### Scenario: Hand-fixed comment resolves without an apply turn
- **WHEN** the user marks a comment as fixed by hand
- **THEN** no apply turn is issued and no confirm-diff is shown for it, and the
  comment is recorded to be resolved at finalize

#### Scenario: The hand-written fix reaches the amend
- **WHEN** a comment is marked as fixed by hand after the user edited the file
  externally
- **THEN** the file is re-read so its current contents are included in the
  finalize amend

### Requirement: Triage ends at an explicit completion gate
The application SHALL require an explicit action to end triage and begin the apply
phase, rather than ending when the last comment is reached, and SHALL report at
that gate how many comments still carry no decision. Undecided comments SHALL
become the skipped fate — no edit, no reply, left as-is — only once the user
confirms the gate.

Deferral during triage means "not yet"; at the gate it becomes "not doing it".
Stating the count is what keeps that transition from happening silently.

#### Scenario: Undecided comments are surfaced before the phase ends
- **WHEN** the user ends triage with comments still undecided
- **THEN** the count of undecided comments is reported and the user confirms
  before the apply phase begins

#### Scenario: Reaching the last comment does not end triage
- **WHEN** the user navigates past the last comment
- **THEN** triage does not end on its own

#### Scenario: Human decision can override the verdict
- **WHEN** Claude's verdict disagrees with a comment but the user accepts it (or
  vice versa)
- **THEN** the user's decision is what governs subsequent phases

#### Scenario: Editing a comment's prose refines the input
- **WHEN** the user edits a comment's prose and then accepts it
- **THEN** the edited prose is what is used when applying the comment
