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

#### Scenario: Hand-fixed comments produce no apply turn
- **WHEN** a comment was marked as fixed by hand during triage
- **THEN** no apply turn is issued for it and no confirm-diff is shown, while its
  file's current contents still participate in the finalize amend

#### Scenario: A later comment's edit sees an earlier confirmed edit
- **WHEN** an apply turn edits a file that an earlier confirmed comment already
  changed
- **THEN** the later turn operates on the already-changed contents

### Requirement: Each apply turn is preceded by a file snapshot
Before issuing an apply turn, the application SHALL snapshot the current contents
of the file that turn will edit. This **pre-turn snapshot** — not the patchset
baseline — is the reference point for both that turn's confirm-diff and its
revert-on-reject. For the first accepted comment in a given file the snapshot
equals the patchset baseline; for every later comment in that file it includes the
edits already confirmed for earlier comments.

#### Scenario: Snapshot is taken before the turn runs
- **WHEN** an apply turn is about to be issued for an accepted comment
- **THEN** the current contents of that comment's file are captured as the turn's
  pre-turn snapshot

#### Scenario: Second comment in a file snapshots the confirmed state
- **WHEN** an apply turn is issued for a comment in a file that already has a
  confirmed edit from an earlier comment
- **THEN** the pre-turn snapshot contains that earlier confirmed edit

### Requirement: Confirm-diff before any write is kept
The application SHALL present each comment's proposed edit as a diff between that
turn's **pre-turn snapshot** and Claude's edit, so the diff shows only the change
attributable to the comment under review, and SHALL require explicit user
confirmation before the edit is kept. No edit *produced by an apply turn* SHALL
become part of the pushed patchset without confirmation.

This gate covers machine-produced edits. Changes the user makes by hand in their
own editor against the worktree are their own review and are not gated here; they
reach the patchset by way of the finalize amend.

#### Scenario: Edit is shown before it is kept
- **WHEN** an apply turn produces an edit for a comment
- **THEN** the edit is shown as a snapshot-vs-edit diff and is not kept until the
  user confirms it

#### Scenario: Confirm-diff excludes earlier confirmed edits
- **WHEN** the confirm-diff is shown for a comment in a file that an earlier
  confirmed comment already changed
- **THEN** the diff shows only the current turn's change, not the earlier
  confirmed edit

### Requirement: Rejecting an edit restores the pre-turn snapshot
The application SHALL restore the affected file to that turn's **pre-turn
snapshot** when the user rejects a proposed edit, since an apply turn writes to
the worktree directly. Restoring the patchset baseline instead would discard edits
already confirmed for earlier comments in the same file, so reject SHALL NOT be
implemented as `git checkout -- <file>`. After a revert the user MAY re-run the
apply turn for that comment with an added hint, or skip the comment.

#### Scenario: Rejected edit is reverted
- **WHEN** the user rejects a proposed edit
- **THEN** the affected file is restored to that turn's pre-turn snapshot and the
  edit does not appear in any later diff or the final patchset

#### Scenario: Reject preserves earlier confirmed edits in the same file
- **WHEN** the user rejects an edit for a comment in a file that an earlier
  confirmed comment already changed
- **THEN** the earlier confirmed edit remains in the worktree

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
