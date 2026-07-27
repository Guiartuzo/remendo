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
The application SHALL fetch a change's current revision and file list and its
inline comments, and SHALL expose the comments that are unresolved, each mapped to
its file, line (or range), author, and message text.

#### Scenario: Unresolved comments are surfaced
- **WHEN** a change's comments are fetched
- **THEN** each unresolved comment is available with its file, line/range, author,
  and prose message

### Requirement: Resolve comments and post replies after push
The application SHALL be able to mark comments resolved and to post reply comments
via Gerrit, and SHALL perform these only after a new patchset has been
successfully pushed. Resolutions and replies for a review SHALL be sent as
batched operations.

#### Scenario: Resolutions wait for a successful push
- **WHEN** the finalize step has amended and pushed a new patchset successfully
- **THEN** the accepted comments are marked resolved and the approved replies are
  posted

#### Scenario: Failed push posts nothing
- **WHEN** the push of the new patchset fails
- **THEN** no comment is marked resolved and no reply is posted

### Requirement: Amend and push a single new patchset
The application SHALL finalize a review by amending all confirmed edits into a
single new commit and pushing it as a new patchset to the change (via the git
`refs/for/<branch>` flow), rather than using the Gerrit change-edit REST API.

#### Scenario: Confirmed edits become one patchset
- **WHEN** the user finalizes a review with one or more confirmed edits staged in
  the worktree
- **THEN** the edits are amended into a single commit and pushed as one new
  patchset
