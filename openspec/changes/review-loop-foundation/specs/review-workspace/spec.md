## ADDED Requirements

### Requirement: Launch by Gerrit change id
The application SHALL accept a Gerrit change id as its launch argument
(`remendo <change-id>`) and, on startup, fetch that change's current revision and
file list, and load its unresolved inline comments, before presenting the review
UI.

#### Scenario: Change is loaded on launch
- **WHEN** the application is launched with a valid change id
- **THEN** the change's current revision, files, and unresolved inline comments
  are fetched and the review UI is presented for that change

#### Scenario: Unknown or inaccessible change
- **WHEN** the application is launched with a change id that does not exist or the
  user cannot access
- **THEN** the application reports the error and exits without creating a worktree

### Requirement: Isolated worktree checkout
The application SHALL check out the change's patchset into a dedicated git
worktree that it owns, rather than mutating the user's existing working tree. The
patchset SHALL be obtained by fetching the change's revision ref and checking it
out into that worktree.

#### Scenario: Worktree is created for the patchset
- **WHEN** a change is loaded
- **THEN** a dedicated worktree is created and the change's patchset revision is
  checked out into it

#### Scenario: User's working tree is untouched
- **WHEN** the application checks out a change
- **THEN** the user's pre-existing working tree and its uncommitted changes are
  not modified

### Requirement: The worktree path is available during the review
The application SHALL make the dedicated worktree's path available to the user
throughout the review, not only on abort. Since v0 has no in-application code
editor, hand-written fixes happen in the user's own editor against that worktree,
which makes its location part of the normal workflow rather than an error path.

#### Scenario: User can locate the worktree mid-review
- **WHEN** the user wants to open a file from the change in their own editor
- **THEN** the worktree path is obtainable from the application without ending the
  review

### Requirement: Abort leaves the worktree intact
When the user aborts a review before finalizing, the application SHALL leave the
dedicated worktree in place with any confirmed-but-unpushed edits, and SHALL NOT
push anything or modify the user's real working tree. The application SHALL report
where the worktree is so the review can be resumed or the worktree removed.

#### Scenario: Aborting mid-review preserves work and pushes nothing
- **WHEN** the user aborts after some edits have been confirmed in the worktree
  but before finalizing
- **THEN** no patchset is pushed, the worktree and its confirmed edits are left in
  place, and the worktree location is reported
