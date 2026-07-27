## ADDED Requirements

### Requirement: Verdict pass over all unresolved comments
On loading a change, the application SHALL run a Claude verdict pass that
adjudicates every unresolved comment, producing for each a verdict
(`agree`/`disagree`/`unsure`) and a justification, before or as the user begins
triage.

#### Scenario: Every unresolved comment receives a verdict
- **WHEN** a change with unresolved comments is loaded
- **THEN** each unresolved comment has an associated verdict and justification
  available to the triage UI

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
