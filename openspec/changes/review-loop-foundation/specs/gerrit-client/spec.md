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

### Requirement: Gerrit credentials are obtained from git
The application SHALL obtain its Gerrit credential by invoking git's credential
protocol (`git credential fill`) for the resolved Gerrit host, and SHALL NOT parse
`.netrc`, read a credential file of its own, or store a secret.

Delegating to git inherits whatever mechanism the user already has working for
`git push` — `.netrc`, an OS keychain, `credential.helper store`, gitcookies, or a
corporate SSO helper — where selecting any single mechanism would serve only the
subset of the team using it. Because the credential arrives through git, it
belongs to the **git** trait rather than the Gerrit trait.

#### Scenario: The credential comes from git's configured helper
- **WHEN** the application needs to authenticate a Gerrit REST request
- **THEN** it obtains the credential through git's credential protocol for that
  host, rather than reading any credential store of its own

#### Scenario: No credential is available
- **WHEN** git's credential protocol yields no usable credential for the host
- **THEN** the application reports the host it asked about and exits, rather than
  issuing an unauthenticated request

### Requirement: TLS trust follows the system and git configuration
The application SHALL validate Gerrit's TLS certificate against the system root
store, and when validation fails SHALL surface `git config --get http.sslCAInfo`
as the place a corporate CA is expected to be configured.

A Gerrit reachable by `git push` but not by Remendo is a trust-store difference,
not an authentication failure, and reporting it as the latter costs the user a
long detour.

#### Scenario: A corporate CA failure names where trust is configured
- **WHEN** the TLS handshake with the Gerrit host fails certificate validation
- **THEN** the error identifies the host and points at git's `http.sslCAInfo`
  setting rather than reporting an authentication or network failure

### Requirement: Fetch change, files, and comment threads
The application SHALL fetch a change's current revision, target branch, and file
list, and its inline comments, and SHALL expose them **grouped into threads**,
each thread mapped to its file, line (or range), the ordered comments it contains
with their authors and message text, and their comment ids. The change's
`current_revision` and `branch` fields SHALL be retained, as finalize depends on
both.

#### Scenario: Unresolved threads are surfaced
- **WHEN** a change's comments are fetched
- **THEN** each unresolved thread is available with its file, line/range, and its
  ordered comments with authors, prose, and comment ids

#### Scenario: Revision and branch are retained for finalize
- **WHEN** a change is fetched
- **THEN** its `current_revision` and `branch` are retained for the pre-push
  revision check and the push refspec

### Requirement: A thread's state is its last comment's flag
The application SHALL determine whether a comment thread is unresolved from the
`unresolved` flag of the **last** comment in that thread, and SHALL NOT treat a
thread as unresolved merely because some earlier comment in it carries
`unresolved: true`. Threads SHALL be assembled by following `in_reply_to` chains;
where a chain branches, the last comment SHALL be the one with the greatest
`updated` timestamp.

Per-comment filtering re-surfaces threads that were explicitly settled: a
reviewer's `unresolved: true` concern followed by a reply carrying
`unresolved: false` is a closed thread, and triaging its opening comment presents
the human with a question already answered.

#### Scenario: A settled thread is not triaged
- **WHEN** a thread's opening comment carries `unresolved: true` and its last
  comment carries `unresolved: false`
- **THEN** the thread is treated as resolved and is not presented for triage

#### Scenario: A branching thread resolves by recency
- **WHEN** two comments in a thread share an `in_reply_to` parent
- **THEN** the comment with the greatest `updated` timestamp determines the
  thread's state

### Requirement: The thread is the unit of triage and reply
The application SHALL treat the thread, not the individual comment, as the unit it
adjudicates and replies to. The thread's **first** comment SHALL supply the
anchor, the **entire exchange** SHALL be supplied as the concern to adjudicate,
and a reply SHALL set `in_reply_to` to the thread's **last** comment id.

An open thread's actual request is frequently not its opening comment: where a
reviewer raises a concern, the author answers it, and the reviewer narrows the ask,
adjudicating only the opening comment argues a point already conceded and drafts a
rebuttal to a settled question.

#### Scenario: The whole exchange is adjudicated
- **WHEN** an unresolved thread contains more than one comment
- **THEN** the verdict turn receives the full ordered exchange, not only the
  opening comment

#### Scenario: A reply targets the end of the thread
- **WHEN** a reply is posted for a rejected thread
- **THEN** its `in_reply_to` names the thread's last comment id

### Requirement: Only current-patchset threads are triaged, and skips are reported
The application SHALL triage only threads anchored on the change's current
patchset, and SHALL report the number of unresolved threads anchored on earlier
patchsets that it did not triage.

A comment's line anchor addresses the revision it was written against. On a change
that has advanced several patchsets, that line may name different code, or code
that no longer exists, and feeding it to an apply turn produces a confident edit in
the wrong place. Reporting the count applies the same rule the `depends_on` field
applies to verdicts: a skipped thing SHALL NOT be indistinguishable from an absent
thing.

#### Scenario: An older thread is skipped visibly
- **WHEN** a change carries unresolved threads anchored on earlier patchsets
- **THEN** those threads are not triaged, and their count is reported to the user

#### Scenario: Current-patchset threads are unaffected
- **WHEN** a change's unresolved threads are all anchored on the current patchset
- **THEN** every one of them is presented for triage and no skip is reported

### Requirement: Comment provenance is filtered explicitly
The application SHALL exclude draft comments from triage, and SHALL include
threads the user themselves started as well as robot comments.

Drafts are unpublished — no reviewer has said them, so adjudicating them acts on
something not yet asked. Robot comments are fetched from Gerrit's separate
robot-comment endpoint and carry a distinct shape, including a robot identifier and
any machine-applicable fix suggestions; they are a second fetch path, not a filter
flag.

Self-authored filtering, were it applied, SHALL be at thread level — a thread the
user *started* — never at comment level. Excluding individual comments the user
wrote would drop their reply out of another reviewer's thread and take that
thread's true state with it.

#### Scenario: Drafts never reach triage
- **WHEN** the user has unpublished draft comments on the change
- **THEN** those drafts are not fetched and are not presented for triage

#### Scenario: Robot comments are fetched and triaged
- **WHEN** a change carries unresolved robot comments
- **THEN** they are fetched from the robot-comment endpoint and presented for
  triage alongside human threads

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
