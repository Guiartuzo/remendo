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

### Requirement: The working directory identifies the repository and the Gerrit
The application SHALL require its working directory to be inside a git clone, and
SHALL derive the Gerrit base URL from that clone's `origin` remote unless an
explicit override is configured. It SHALL validate the fetched change's `project`
field against the clone and SHALL refuse to proceed on a mismatch, naming both the
change's project and the clone's.

A change id identifies no repository, so the launch argument alone cannot say which
clone to operate on, which Gerrit to query, or which credential to present.
Requiring cwd to be inside a clone supplies all three, and matches how `git` itself
resolves context.

Derivation is a default, not a guarantee: a Gerrit served under a URL subpath, or
reached over SSH on a non-default port, is not always recoverable from the remote.
Failures SHALL therefore report the base URL that was derived, since a bare HTTP
error naming no host is not actionable.

#### Scenario: Launch outside a clone is refused
- **WHEN** the application is launched with its working directory not inside a git
  clone
- **THEN** it reports the error and exits without creating a worktree

#### Scenario: A change from another project is refused
- **WHEN** the fetched change's `project` does not match the clone the application
  was launched in
- **THEN** it reports both the change's project and the clone's, and exits without
  creating a worktree

#### Scenario: A derivation failure names what it tried
- **WHEN** the Gerrit base URL derived from the `origin` remote does not serve the
  REST API
- **THEN** the reported error contains the derived base URL

### Requirement: Isolated worktree checkout
The application SHALL check out the change's patchset into a dedicated git
worktree that it owns, rather than mutating the user's existing working tree. The
patchset SHALL be obtained by fetching the change's revision ref and checking it
out into that worktree. The worktree SHALL live outside the clone, under the user's
state directory, keyed by project and change id — so it survives `git clean` and
does not pollute the repository.

#### Scenario: Worktree is created for the patchset
- **WHEN** a change is loaded
- **THEN** a dedicated worktree is created and the change's patchset revision is
  checked out into it

#### Scenario: User's working tree is untouched
- **WHEN** the application checks out a change
- **THEN** the user's pre-existing working tree and its uncommitted changes are
  not modified

### Requirement: Relaunching a change reuses its worktree
The application SHALL reuse an existing worktree when it is launched for a change
that already has one, preserving its confirmed-but-unpushed edits, rather than
recreating it or refusing to start. Because abort deliberately leaves the worktree
in place, the second launch against any change is a resume, and that is the common
path rather than an exceptional one.

#### Scenario: A relaunch resumes rather than discards
- **WHEN** the application is launched for a change that already has a worktree
  containing confirmed edits
- **THEN** that worktree is reused and its edits are preserved

### Requirement: Verdicts are cached across relaunches
The application SHALL persist each change's verdict payloads, keyed by change id
**and revision**, and SHALL reuse them on relaunch instead of re-running the verdict
pass. A change whose current revision differs from the cached one SHALL be treated
as a cache miss. The per-turn cost reported by the driver SHALL be accumulated
alongside the cached verdicts.

Re-running the pass is not only a repeated charge but a source of inconsistency: a
second pass returns different verdicts over a worktree that already contains the
first pass's applied fixes. Human triage decisions are deliberately *not* cached —
they are cheap to redo, where verdicts are not. Keying on revision is what keeps
the cache from describing code that no longer exists.

#### Scenario: A relaunch reuses cached verdicts
- **WHEN** the application relaunches for a change whose revision is unchanged and
  whose verdicts were cached
- **THEN** the cached verdicts are loaded and no verdict turn is issued

#### Scenario: A new patchset invalidates the cache
- **WHEN** the application relaunches for a change whose current revision differs
  from the revision the cached verdicts were produced against
- **THEN** the cached verdicts are not used and the verdict pass runs again

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
