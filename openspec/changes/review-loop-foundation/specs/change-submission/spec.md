## ADDED Requirements

### Requirement: Finalize amends and pushes once
The application SHALL finalize a review by folding all confirmed edits into a
single amended commit and pushing it as one new patchset. If the push is
rejected, the application SHALL surface the failure, leave the worktree intact,
and take no further Gerrit action.

#### Scenario: One push for all confirmed edits
- **WHEN** the user finalizes with confirmed edits staged
- **THEN** the edits are amended into a single commit and pushed as one new
  patchset

#### Scenario: Push rejection halts finalize safely
- **WHEN** the push is rejected by Gerrit
- **THEN** the failure is reported, the worktree is left intact, and no comment is
  resolved and no reply is posted

### Requirement: Resolve accepted comments after a successful push
Following a successful push, the application SHALL mark the accepted-and-applied
comments resolved, as a batch.

#### Scenario: Accepted comments resolved post-push
- **WHEN** the new patchset has been pushed successfully
- **THEN** the comments whose accepted edits are in that patchset are marked
  resolved

### Requirement: Post drafted replies for rejected comments
For comments the user rejected, the application SHALL let Claude draft a reply
explaining the disagreement, require the user to approve it, and post approved
replies only after a successful push. Rejected comments that receive a reply
SHALL remain unresolved.

#### Scenario: Rejected comment gets an approved reply
- **WHEN** the user rejects a comment, approves Claude's drafted reply, and the
  push succeeds
- **THEN** the reply is posted to that comment and the comment is left unresolved

#### Scenario: Unapproved reply is not posted
- **WHEN** the user does not approve a drafted reply
- **THEN** no reply is posted for that comment
