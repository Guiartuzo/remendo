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

### Requirement: Two-panel triage view
The application SHALL present triage in two panels: one showing the code with its
associated reviewer comment and the comment's **author name**, and the other
showing Claude's verdict and justification for that comment. The author name is
shown for the human's judgment only; it SHALL NOT be provided to Claude for the
verdict (the verdict is judged on technical merit).

#### Scenario: Code, comment, author, and verdict are shown together
- **WHEN** the user is triaging a comment
- **THEN** the code, the reviewer comment, and the comment's author name are shown
  alongside Claude's verdict and justification for the same comment

#### Scenario: Verdict is not influenced by the author
- **WHEN** the verdict pass adjudicates a comment
- **THEN** the comment's author identity is not part of the input that produces
  the verdict

### Requirement: Manual accept, reject, and edit
The application SHALL let the user, per comment, accept it, reject it, or edit its
prose before it is used, and SHALL navigate between comments. v0 SHALL provide
manual triage only, with no automatic acceptance or rejection.

#### Scenario: User decides each comment
- **WHEN** the user acts on a comment during triage
- **THEN** the comment is recorded as accepted, rejected, or edited according to
  the user's choice

#### Scenario: Human decision can override the verdict
- **WHEN** Claude's verdict disagrees with a comment but the user accepts it (or
  vice versa)
- **THEN** the user's decision is what governs subsequent phases

#### Scenario: Editing a comment's prose refines the input
- **WHEN** the user edits a comment's prose and then accepts it
- **THEN** the edited prose is what is used when applying the comment
