//! Claude's adjudication of one comment thread.
//!
//! The schema was settled in the decisions session (design.md §13, `tasks.md`
//! 4.3), and two of its properties are load-bearing:
//!
//! * **`depends_on` is required and nullable, never omittable.** Requiring it
//!   makes "I had no way to know this" a stated position rather than a silent
//!   omission, and makes a fabricated value — an invented ticket number, an
//!   assumed convention — unreachable without first declaring the gap.
//! * **There is no `confidence` field.** The dry run measured 9 of 12
//!   context-dependent verdicts filed as *confident*, so a self-reported grade
//!   pointed away from the property the human needs. `depends_on` carries the
//!   same signal in a form the model cannot inflate.

use serde::{Deserialize, Deserializer, Serialize};

/// Deserialize a field that is **nullable but not omittable**.
///
/// serde's derive treats a bare `Option<T>` as optional: a missing field
/// silently becomes `None`, which is exactly the silent omission `depends_on`
/// exists to prevent. Routing it through `deserialize_with` makes serde emit a
/// missing-field error instead, so the field must be written out — as a value
/// or as an explicit `null`.
fn required_nullable<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::deserialize(deserializer)
}

/// How Claude adjudicated a comment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Adjudication {
    /// A real issue here, worth addressing in this change.
    Agree,
    /// No real issue — moot, or the reviewer was wrong.
    Disagree,
    /// Needs human judgment, including when the comment depends on context the
    /// code cannot show.
    Unsure,
}

impl Adjudication {
    /// A short label for the triage panel.
    pub fn label(self) -> &'static str {
        match self {
            Adjudication::Agree => "agree",
            Adjudication::Disagree => "disagree",
            Adjudication::Unsure => "unsure",
        }
    }
}

/// What kind of out-of-code fact a verdict rests on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DependencyKind {
    CiConfig,
    ToolVersion,
    TeamConvention,
    Roadmap,
    Ticket,
    Other,
}

/// A fact outside the code that a verdict rests on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Dependency {
    /// What is unknown.
    pub fact: String,
    pub kind: DependencyKind,
    /// How a **human** could settle it.
    ///
    /// Never executed. Remendo runs nothing from this field — doing so would
    /// reintroduce arbitrary command execution from model output immediately
    /// after apply turns were restricted to read and edit tooling, and the dry
    /// run's one measured self-clearing dependency was a probe invoking the
    /// `claude` CLI itself (design.md §13).
    pub verify: String,
    /// What the verdict becomes if the fact resolves the other way. The
    /// actionable half: "if the trailer convention exists, this flips to agree."
    #[serde(default)]
    pub flips_to: Option<Adjudication>,
}

/// One thread's verdict.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Verdict {
    /// The id of the thread's opening comment.
    pub comment_id: String,
    pub verdict: Adjudication,
    pub justification: String,
    /// Facts outside the code this verdict rests on, or `null` when the code
    /// alone settles it. An **array**: one verdict can rest on several facts,
    /// and the dry run's 12 verdicts collapsed onto 10 distinct ones.
    ///
    /// Required and nullable, never omittable — see [`required_nullable`].
    #[serde(deserialize_with = "required_nullable")]
    pub depends_on: Option<Vec<Dependency>>,
}

impl Verdict {
    /// The dependencies this verdict declares, empty when it declared none.
    pub fn dependencies(&self) -> &[Dependency] {
        self.depends_on.as_deref().unwrap_or(&[])
    }

    /// Whether the verdict claims to be decidable from the code alone.
    ///
    /// ```
    /// # use remendo::verdict::Verdict;
    /// let v: Verdict = serde_json::from_str(
    ///     r#"{"comment_id":"c1","verdict":"agree","justification":"…","depends_on":null}"#
    /// ).unwrap();
    /// assert!(v.is_self_contained());
    /// ```
    pub fn is_self_contained(&self) -> bool {
        self.dependencies().is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(json: &str) -> Result<Verdict, serde_json::Error> {
        serde_json::from_str(json)
    }

    #[test]
    fn a_self_contained_verdict_parses_with_a_null_dependency() {
        let v = parse(
            r#"{"comment_id":"c1","verdict":"agree","justification":"Real bug.",
                "depends_on":null}"#,
        )
        .unwrap();
        assert_eq!(v.verdict, Adjudication::Agree);
        assert!(v.is_self_contained());
        assert_eq!(v.dependencies().len(), 0);
    }

    #[test]
    fn a_verdict_resting_on_several_facts_keeps_them_all() {
        let v = parse(
            r#"{"comment_id":"c1","verdict":"unsure","justification":"…",
                "depends_on":[
                  {"fact":"the Bug: trailer convention","kind":"team-convention",
                   "verify":"ask the team or read CONTRIBUTING.md","flips_to":"agree"},
                  {"fact":"the ticket number","kind":"ticket",
                   "verify":"look it up in the tracker","flips_to":null}
                ]}"#,
        )
        .unwrap();
        assert_eq!(v.dependencies().len(), 2);
        assert!(!v.is_self_contained());
        assert_eq!(v.dependencies()[0].flips_to, Some(Adjudication::Agree));
        assert_eq!(v.dependencies()[1].flips_to, None);
    }

    /// The forcing function only works if the field cannot be dropped.
    #[test]
    fn a_verdict_omitting_depends_on_is_rejected() {
        let err = parse(r#"{"comment_id":"c1","verdict":"agree","justification":"…"}"#)
            .expect_err("depends_on is required, not merely nullable");
        assert!(err.to_string().contains("depends_on"), "{err}");
    }

    /// The field was specified, then deliberately removed. If a payload carries
    /// one it is ignored rather than surfaced — nothing may render it.
    #[test]
    fn a_confidence_field_is_not_modelled() {
        let v = parse(
            r#"{"comment_id":"c1","verdict":"agree","justification":"…",
                "depends_on":null,"confidence":"high"}"#,
        )
        .unwrap();
        let round_tripped = serde_json::to_value(&v).unwrap();
        assert!(
            round_tripped.get("confidence").is_none(),
            "confidence must not survive into anything the UI can read"
        );
    }

    #[test]
    fn every_adjudication_round_trips() {
        for (json, value) in [
            ("\"agree\"", Adjudication::Agree),
            ("\"disagree\"", Adjudication::Disagree),
            ("\"unsure\"", Adjudication::Unsure),
        ] {
            assert_eq!(serde_json::from_str::<Adjudication>(json).unwrap(), value);
            assert_eq!(serde_json::to_string(&value).unwrap(), json);
        }
    }

    #[test]
    fn dependency_kinds_use_kebab_case_on_the_wire() {
        let kind: DependencyKind = serde_json::from_str("\"team-convention\"").unwrap();
        assert_eq!(kind, DependencyKind::TeamConvention);
        assert_eq!(
            serde_json::to_string(&DependencyKind::CiConfig).unwrap(),
            "\"ci-config\""
        );
    }

    #[test]
    fn an_unknown_adjudication_is_rejected_rather_than_defaulted() {
        assert!(serde_json::from_str::<Adjudication>("\"maybe\"").is_err());
    }
}
