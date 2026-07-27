## ADDED Requirements

### Requirement: Per-comment apply of accepted comments
After triage, the application SHALL apply accepted comments one comment at a
time, issuing a scoped apply turn to the resumed Claude session for each accepted
comment. Successive apply turns operate on the same worktree, so an edit for one
comment observes the edits already confirmed for earlier comments.

#### Scenario: Each accepted comment gets its own apply turn
- **WHEN** the apply phase processes an accepted comment
- **THEN** a scoped apply turn is issued for that single comment to produce its
  edit

#### Scenario: Rejected and skipped comments produce no edit
- **WHEN** a comment was rejected or skipped during triage
- **THEN** no apply turn is issued for it

#### Scenario: A later comment's edit sees an earlier confirmed edit
- **WHEN** an apply turn edits a file that an earlier confirmed comment already
  changed
- **THEN** the later turn operates on the already-changed contents

### Requirement: Confirm-diff before any write is kept
The application SHALL present each comment's proposed edit as a diff between the
patchset baseline and Claude's edit, and SHALL require explicit user confirmation
before the edit is kept. Nothing SHALL become part of the pushed patchset without
confirmation.

#### Scenario: Edit is shown before it is kept
- **WHEN** an apply turn produces an edit for a comment
- **THEN** the edit is shown as a patchset-vs-edit diff and is not kept until the
  user confirms it

### Requirement: Rejecting an edit reverts the file
The application SHALL restore the affected file to its patchset-baseline contents
when the user rejects a proposed edit, since an apply turn writes to the worktree
directly. After a revert the user MAY re-run the apply turn for that comment with
an added hint, or skip the comment.

#### Scenario: Rejected edit is reverted
- **WHEN** the user rejects a proposed edit
- **THEN** the affected file is restored to its patchset-baseline contents and the
  edit does not appear in any later diff or the final patchset

#### Scenario: Re-run after reject refines the edit
- **WHEN** the user rejects an edit and re-runs the apply turn with an added hint
- **THEN** a new apply turn is issued for the same comment and its result is shown
  for confirmation

### Requirement: Confirmed edits accumulate in the worktree
On confirmation, the application SHALL keep the edit in the dedicated worktree,
accumulating confirmed edits across comments until finalize. Confirmed edits
SHALL NOT be pushed at confirmation time.

#### Scenario: Confirmed edits are retained until finalize
- **WHEN** the user confirms edits for several comments
- **THEN** those edits remain in the worktree and unpushed until the finalize step
