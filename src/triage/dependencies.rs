//! Collapsing repeated `depends_on` facts into one shared dependency.
//!
//! The dry run measured **12 verdicts resting on 10 distinct facts**, one fact
//! covering three verdicts. Presented per verdict, the human reads the same
//! "I could not know X" three times and treats it as noise; presented once, it
//! is a short go-find-out list.
//!
//! Aggregation lives here rather than in the schema on purpose (design.md §14):
//! the schema stays per-verdict, and the collapsing is a presentation concern.

use crate::verdict::{Adjudication, Dependency, DependencyKind, Verdict};

/// One out-of-code fact, and every verdict resting on it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SharedDependency {
    pub fact: String,
    pub kind: DependencyKind,
    /// How a human could settle it. **Never executed** — rendered as text with
    /// no action attached (design.md §13).
    pub verify: String,
    /// Comment ids of the verdicts resting on this fact, in first-seen order.
    pub comment_ids: Vec<String>,
    /// What the affected verdicts would become. `Some` only when every verdict
    /// resting on this fact agrees on the outcome; conflicting claims collapse
    /// to `None` rather than picking one.
    pub flips_to: Option<Adjudication>,
}

impl SharedDependency {
    /// How many verdicts rest on this fact.
    pub fn verdict_count(&self) -> usize {
        self.comment_ids.len()
    }

    /// Whether more than one verdict rests on it — the case worth collapsing.
    pub fn is_shared(&self) -> bool {
        self.comment_ids.len() > 1
    }
}

/// Collate every verdict's dependencies into a deduplicated list.
///
/// Facts are matched by their text. Order is first-seen, so the list reads in
/// the order the human will meet the verdicts.
pub fn collate(verdicts: &[Verdict]) -> Vec<SharedDependency> {
    let mut collated: Vec<SharedDependency> = Vec::new();
    for verdict in verdicts {
        for dependency in verdict.dependencies() {
            match collated.iter_mut().find(|d| d.fact == dependency.fact) {
                Some(existing) => merge_into(existing, dependency, &verdict.comment_id),
                None => collated.push(SharedDependency {
                    fact: dependency.fact.clone(),
                    kind: dependency.kind,
                    verify: dependency.verify.clone(),
                    comment_ids: vec![verdict.comment_id.clone()],
                    flips_to: dependency.flips_to,
                }),
            }
        }
    }
    collated
}

/// Fold another verdict's claim about the same fact into an existing entry.
fn merge_into(existing: &mut SharedDependency, dependency: &Dependency, comment_id: &str) {
    if !existing.comment_ids.iter().any(|id| id == comment_id) {
        existing.comment_ids.push(comment_id.to_string());
    }
    // Two verdicts can disagree about what settling the fact would do. Showing
    // one of them would be inventing a consensus, so disagreement erases the
    // claim instead.
    if existing.flips_to != dependency.flips_to {
        existing.flips_to = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn verdict(id: &str, deps: serde_json::Value) -> Verdict {
        serde_json::from_value(serde_json::json!({
            "comment_id": id, "verdict": "unsure",
            "justification": "…", "depends_on": deps,
        }))
        .unwrap()
    }

    fn dep(fact: &str, flips_to: Option<&str>) -> serde_json::Value {
        serde_json::json!({
            "fact": fact, "kind": "team-convention",
            "verify": format!("settle: {fact}"), "flips_to": flips_to,
        })
    }

    #[test]
    fn a_fact_shared_by_three_verdicts_is_presented_once() {
        let verdicts = vec![
            verdict(
                "c1",
                serde_json::json!([dep("the Bug: trailer convention", None)]),
            ),
            verdict(
                "c2",
                serde_json::json!([dep("the Bug: trailer convention", None)]),
            ),
            verdict(
                "c3",
                serde_json::json!([dep("the Bug: trailer convention", None)]),
            ),
        ];
        let collated = collate(&verdicts);
        assert_eq!(collated.len(), 1, "one entry, not three");
        assert_eq!(collated[0].verdict_count(), 3);
        assert_eq!(collated[0].comment_ids, vec!["c1", "c2", "c3"]);
        assert!(collated[0].is_shared());
    }

    #[test]
    fn distinct_facts_stay_distinct() {
        let verdicts = vec![
            verdict("c1", serde_json::json!([dep("the CI matrix", None)])),
            verdict("c2", serde_json::json!([dep("the rustc version", None)])),
        ];
        assert_eq!(collate(&verdicts).len(), 2);
    }

    #[test]
    fn one_verdict_resting_on_two_facts_contributes_to_both() {
        let verdicts = vec![verdict(
            "c1",
            serde_json::json!([dep("the CI matrix", None), dep("the rustc version", None)]),
        )];
        let collated = collate(&verdicts);
        assert_eq!(collated.len(), 2);
        assert!(collated.iter().all(|d| d.comment_ids == vec!["c1"]));
    }

    #[test]
    fn self_contained_verdicts_contribute_nothing() {
        let verdicts = vec![verdict("c1", serde_json::Value::Null)];
        assert!(collate(&verdicts).is_empty());
    }

    #[test]
    fn a_shared_flips_to_survives_when_every_verdict_agrees() {
        let verdicts = vec![
            verdict(
                "c1",
                serde_json::json!([dep("the convention", Some("agree"))]),
            ),
            verdict(
                "c2",
                serde_json::json!([dep("the convention", Some("agree"))]),
            ),
        ];
        let collated = collate(&verdicts);
        assert_eq!(collated[0].flips_to, Some(Adjudication::Agree));
    }

    /// Showing one of two conflicting claims would invent a consensus.
    #[test]
    fn conflicting_flips_to_claims_collapse_to_none() {
        let verdicts = vec![
            verdict(
                "c1",
                serde_json::json!([dep("the convention", Some("agree"))]),
            ),
            verdict(
                "c2",
                serde_json::json!([dep("the convention", Some("disagree"))]),
            ),
        ];
        let collated = collate(&verdicts);
        assert_eq!(collated[0].flips_to, None);
        assert_eq!(collated[0].verdict_count(), 2, "still shared");
    }

    #[test]
    fn the_verify_text_is_carried_through_for_display() {
        let verdicts = vec![verdict(
            "c1",
            serde_json::json!([dep("the CI matrix", None)]),
        )];
        assert_eq!(collate(&verdicts)[0].verify, "settle: the CI matrix");
    }

    #[test]
    fn a_verdict_declaring_the_same_fact_twice_is_counted_once() {
        let verdicts = vec![verdict(
            "c1",
            serde_json::json!([dep("the CI matrix", None), dep("the CI matrix", None)]),
        )];
        let collated = collate(&verdicts);
        assert_eq!(collated.len(), 1);
        assert_eq!(collated[0].verdict_count(), 1, "one verdict, not two");
    }

    #[test]
    fn order_is_first_seen() {
        let verdicts = vec![
            verdict("c1", serde_json::json!([dep("second-met fact", None)])),
            verdict("c2", serde_json::json!([dep("first-met fact", None)])),
        ];
        let collated = collate(&verdicts);
        assert_eq!(collated[0].fact, "second-met fact");
    }
}
