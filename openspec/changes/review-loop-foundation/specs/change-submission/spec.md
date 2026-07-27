## ADDED Requirements

### Requirement: Abort finalize if the change moved
Immediately before pushing, the application SHALL re-fetch the change and compare
its `current_revision` against the revision checked out into the worktree, and
SHALL abort the finalize without pushing if they differ. Human triage can take
long enough for the author to upload a new patchset in the meantime; amending onto
a stale revision would push a patchset that silently reverts their work.

#### Scenario: Revision unchanged, push proceeds
- **WHEN** finalize re-fetches the change and `current_revision` still matches the
  revision checked out in the worktree
- **THEN** the amend and push proceed

#### Scenario: A newer patchset was uploaded during triage
- **WHEN** finalize re-fetches the change and `current_revision` differs from the
  revision checked out in the worktree
- **THEN** the finalize is aborted, nothing is pushed, no comment is resolved, no
  reply is posted, and the divergence is reported to the user with both revisions

### Requirement: Finalize amends and pushes once
The application SHALL finalize a review by folding all confirmed edits into a
single amended commit and pushing it as one new patchset to `refs/for/<branch>`,
where `<branch>` is the change's target branch as reported by Gerrit (the
`branch` field of the change), since the worktree is on a detached HEAD with no
local branch to read. Confirmed edits SHALL be staged before the amend, because
`git commit --amend` over an unstaged working tree amends only the commit message.
If the push is rejected, the application SHALL surface the failure, leave the
worktree intact, and take no further Gerrit action.

#### Scenario: One push for all confirmed edits
- **WHEN** the user finalizes with confirmed edits in the worktree
- **THEN** those edits are staged, amended into a single commit, and pushed as one
  new patchset to the change's target branch

#### Scenario: Push rejection halts finalize safely
- **WHEN** the push is rejected by Gerrit
- **THEN** the failure is reported, the worktree is left intact, and no comment is
  resolved and no reply is posted

### Requirement: Commit-message comments are applied to the amended message
The application SHALL route an accepted `/COMMIT_MSG` comment to the text of the
finalize amend's commit message rather than issuing a file-edit apply turn, and
the amend SHALL therefore be capable of rewriting the message. Comments anchored
on Gerrit's `/COMMIT_MSG` pseudo-path refer to the change's commit message, not to
a file on disk, so there is nothing on disk to edit.

#### Scenario: Accepted commit-message comment rewrites the message
- **WHEN** a comment anchored on `/COMMIT_MSG` is accepted during triage
- **THEN** the finalize amend applies it to the commit message and no file edit is
  attempted for it

### Requirement: One batched review post finalizes every comment fate
The application SHALL finalize all comment fates — resolutions of accepted
comments and approved replies to rejected comments — as a **single** batched
review POST issued only after a successful push, with each comment's `unresolved`
flag carrying its fate. Rejected comments that receive a reply SHALL remain
unresolved. This is one operation because Gerrit exposes no mark-comment-resolved
mutation: a thread is resolved by replying to it with `unresolved: false`.

#### Scenario: Accepted and rejected comments settle in one call
- **WHEN** the new patchset has been pushed successfully
- **THEN** one batched review post marks the accepted comments resolved and posts
  the approved replies to the rejected comments, leaving those rejected comments
  unresolved

#### Scenario: Hand-fixed comments resolve alongside applied ones
- **WHEN** the pushed patchset includes a fix the user wrote by hand for a comment
  they marked as fixed by hand
- **THEN** that comment is carried in the same batched post with
  `unresolved: false`, indistinguishably from a comment whose fix came from an
  apply turn

#### Scenario: Skipped comments are left alone
- **WHEN** comments were skipped during triage
- **THEN** they are omitted from the batched post entirely, so they are neither
  resolved nor replied to

#### Scenario: Failed push settles nothing
- **WHEN** the push of the new patchset fails
- **THEN** the batched review post is not issued, so no comment is resolved and no
  reply is posted

#### Scenario: Unapproved reply is not posted
- **WHEN** the user did not approve a drafted reply for a rejected comment
- **THEN** that comment is omitted from the batched review post and stays
  untouched
