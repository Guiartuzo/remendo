## ADDED Requirements

### Requirement: Gerrit access on a background worker thread
The application SHALL perform all Gerrit REST access on a background worker
thread using a blocking HTTP client, delivering results to the UI over a channel.
The synchronous render loop SHALL NOT block waiting on the network, and the
application SHALL NOT introduce an async runtime for Gerrit access.

#### Scenario: Network work does not block the UI
- **WHEN** a Gerrit request is in flight
- **THEN** the UI remains responsive and receives the result as an event on its
  channel when the request completes

### Requirement: XSSI prefix stripping
The application SHALL strip Gerrit's `)]}'` XSSI guard line from REST response
bodies before parsing them as JSON.

#### Scenario: Guarded response parses correctly
- **WHEN** a Gerrit REST response body begins with the `)]}'` guard line
- **THEN** the guard line is removed and the remaining body is parsed as JSON

### Requirement: Fetch change, files, and unresolved comments
The application SHALL fetch a change's current revision, target branch, and file
list, and its inline comments, and SHALL expose the comments that are unresolved,
each mapped to its file, line (or range), author, message text, and comment id.
The change's `current_revision` and `branch` fields SHALL be retained, as finalize
depends on both.

#### Scenario: Unresolved comments are surfaced
- **WHEN** a change's comments are fetched
- **THEN** each unresolved comment is available with its file, line/range, author,
  prose message, and comment id

#### Scenario: Revision and branch are retained for finalize
- **WHEN** a change is fetched
- **THEN** its `current_revision` and `branch` are retained for the pre-push
  revision check and the push refspec

### Requirement: Comment pseudo-paths are classified, not treated as files
The application SHALL classify a comment's anchor as a real file path, a
commit-message comment, or a change-level comment, and SHALL NOT derive an on-disk
path or a containing directory from the pseudo-paths. Gerrit anchors
commit-message comments on `/COMMIT_MSG` and change-level comments on
`/PATCHSET_LEVEL`; neither is a file on disk.

Line numbers on `/COMMIT_MSG` address Gerrit's synthetic rendering of the commit
message, which prepends header lines (`Parent`, `Author`, `AuthorDate`, `Commit`,
`CommitDate`, then a blank line) before the subject. Mapping such a line to the
real message SHALL account for that offset.

#### Scenario: A commit-message comment is classified as such
- **WHEN** a fetched comment is anchored on `/COMMIT_MSG`
- **THEN** it is exposed as a commit-message comment, with its line resolved
  against the real message rather than Gerrit's synthetic header offset

#### Scenario: A change-level comment is classified as such
- **WHEN** a fetched comment is anchored on `/PATCHSET_LEVEL`
- **THEN** it is exposed as a change-level comment with no associated file path

### Requirement: Comment fates are posted as one review call
The application SHALL express both comment fates — resolution and reply — as a
**single** batched review post against the change's current revision, issued only
after a new patchset has been successfully pushed. Gerrit provides no
mark-comment-resolved mutation: a thread is resolved by posting a reply carrying
`unresolved: false`, so resolving and replying are the same operation differing
only in that flag.

Modelling them as two separate calls would be modelling a mutation that does not
exist, and would open a partial-failure window where resolutions land and replies
do not.

#### Scenario: Resolutions and replies are one call after a successful push
- **WHEN** the finalize step has amended and pushed a new patchset successfully
- **THEN** one batched review post carries the accepted comments with
  `unresolved: false` and the approved replies to rejected comments with
  `unresolved: true`

#### Scenario: Failed push posts nothing
- **WHEN** the push of the new patchset fails
- **THEN** the batched review post is not issued, so no comment is resolved and no
  reply is posted

### Requirement: Amend and push a single new patchset
The application SHALL finalize a review by amending all confirmed edits into a
single new commit and pushing it as a new patchset to the change, rather than
using the Gerrit change-edit REST API. The push refspec SHALL be
`refs/for/<branch>` where `<branch>` is the change's `branch` field from the REST
response; the worktree is on a detached HEAD checked out from `refs/changes/...`,
so there is no local branch name to read.

#### Scenario: Confirmed edits become one patchset
- **WHEN** the user finalizes a review with one or more confirmed edits in the
  worktree
- **THEN** the edits are amended into a single commit and pushed as one new
  patchset to `refs/for/<branch>` using the change's target branch
