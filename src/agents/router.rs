use super::parse_json;
use super::prompts;
use crate::config::ReviewConfig;
use crate::llm::LlmClient;
use crate::repository::is_agent_instructions;
use crate::types::{ReviewAgent, ReviewBundle, ReviewManifest};
use anyhow::{bail, Context, Result};
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug)]
pub struct RoutingAssignment {
    pub agent: ReviewAgent,
    pub bundle_ids: Vec<String>,
    pub rationale: String,
}

#[derive(Clone, Debug)]
pub struct RoutingDecision {
    pub assignments: Vec<RoutingAssignment>,
    pub fallback: bool,
}

pub async fn route(
    client: &LlmClient,
    manifest: &ReviewManifest,
    bundles: &[ReviewBundle],
    config: &ReviewConfig,
) -> RoutingDecision {
    let prompt = prompts::router_prompt(bundles, &manifest.files);
    let result = client
        .run_agent(
            &config.review_model,
            prompts::router_system(),
            &prompt,
            Vec::new(),
            1,
            |_name, _arguments| async { bail!("the routing agent has no repository tools") },
        )
        .await
        .and_then(|raw| parse_routing(&raw, bundles));
    match result {
        Ok(assignments) => RoutingDecision {
            assignments,
            fallback: false,
        },
        Err(error) => {
            eprintln!("specialist routing failed; running every specialist: {error:#}");
            RoutingDecision {
                assignments: fallback_assignments(bundles),
                fallback: true,
            }
        }
    }
}

fn parse_routing(raw: &str, bundles: &[ReviewBundle]) -> Result<Vec<RoutingAssignment>> {
    let response: RoutingResponse = parse_json(raw).context("parse specialist routing")?;
    if response.assignments.is_empty() {
        bail!("router returned no specialist assignments");
    }
    let bundle_by_id = bundles
        .iter()
        .map(|bundle| (bundle.id.as_str(), bundle))
        .collect::<BTreeMap<_, _>>();
    let mut merged = BTreeMap::<ReviewAgent, (BTreeSet<String>, Vec<String>)>::new();
    for assignment in response.assignments {
        if assignment.agent == ReviewAgent::Correctness {
            bail!("router cannot assign the always-on correctness agent");
        }
        if assignment.bundle_ids.is_empty() {
            bail!("router assignment for {} has no bundles", assignment.agent);
        }
        if assignment.rationale.trim().is_empty() {
            bail!(
                "router assignment for {} has no rationale",
                assignment.agent
            );
        }
        let entry = merged.entry(assignment.agent).or_default();
        for id in assignment.bundle_ids {
            let Some(bundle) = bundle_by_id.get(id.as_str()) else {
                bail!("router returned unknown bundle '{id}'");
            };
            if assignment.agent == ReviewAgent::Documentation
                && bundle.paths.iter().all(|path| is_agent_instructions(path))
            {
                continue;
            }
            entry.0.insert(id);
        }
        entry.1.push(assignment.rationale.trim().to_owned());
    }
    let assignments = merged
        .into_iter()
        .filter_map(|(agent, (bundle_ids, rationales))| {
            if bundle_ids.is_empty() {
                return None;
            }
            Some(RoutingAssignment {
                agent,
                bundle_ids: bundle_ids.into_iter().collect(),
                rationale: rationales.join("; "),
            })
        })
        .collect::<Vec<_>>();
    if assignments.is_empty() {
        bail!("router returned no usable specialist assignments");
    }
    Ok(assignments)
}

fn fallback_assignments(bundles: &[ReviewBundle]) -> Vec<RoutingAssignment> {
    let bundle_ids = bundles
        .iter()
        .map(|bundle| bundle.id.clone())
        .collect::<Vec<_>>();
    ReviewAgent::SPECIALISTS
        .into_iter()
        .filter_map(|agent| {
            let assigned = if agent == ReviewAgent::Documentation {
                bundles
                    .iter()
                    .filter(|bundle| bundle.paths.iter().any(|path| !is_agent_instructions(path)))
                    .map(|bundle| bundle.id.clone())
                    .collect()
            } else {
                bundle_ids.clone()
            };
            if assigned.is_empty() {
                return None;
            }
            Some(RoutingAssignment {
                agent,
                bundle_ids: assigned,
                rationale: "Router failed, so PRBot ran every specialist.".to_owned(),
            })
        })
        .collect()
}

#[derive(Deserialize)]
struct RoutingResponse {
    assignments: Vec<RoutingAssignmentResponse>,
}

#[derive(Deserialize)]
struct RoutingAssignmentResponse {
    agent: ReviewAgent,
    bundle_ids: Vec<String>,
    rationale: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::Budget;
    use crate::types::RiskLevel;
    use std::sync::Arc;

    fn bundles() -> Vec<ReviewBundle> {
        vec![
            ReviewBundle {
                id: "api".to_owned(),
                paths: vec!["src/api.rs".to_owned()],
                hunk_count: 1,
                risk: RiskLevel::High,
                related_files: Vec::new(),
            },
            ReviewBundle {
                id: "docs".to_owned(),
                paths: vec!["README.md".to_owned()],
                hunk_count: 1,
                risk: RiskLevel::Low,
                related_files: Vec::new(),
            },
        ]
    }

    #[test]
    fn validates_merges_and_deduplicates_assignments() {
        let raw = r#"{"assignments":[
            {"agent":"security","bundle_ids":["api"],"rationale":"auth changed"},
            {"agent":"security","bundle_ids":["api","docs"],"rationale":"example exposes input"},
            {"agent":"documentation","bundle_ids":["docs"],"rationale":"public docs changed"}
        ]}"#;
        let assignments = parse_routing(raw, &bundles()).expect("routing");
        assert_eq!(assignments.len(), 2);
        let security = assignments
            .iter()
            .find(|assignment| assignment.agent == ReviewAgent::Security)
            .expect("security");
        assert_eq!(security.bundle_ids, ["api", "docs"]);
    }

    #[test]
    fn rejects_unknown_bundles_and_correctness_assignment() {
        let unknown =
            r#"{"assignments":[{"agent":"security","bundle_ids":["missing"],"rationale":"x"}]}"#;
        assert!(parse_routing(unknown, &bundles()).is_err());
        let correctness =
            r#"{"assignments":[{"agent":"correctness","bundle_ids":["api"],"rationale":"x"}]}"#;
        assert!(parse_routing(correctness, &bundles()).is_err());
    }

    #[test]
    fn rejects_empty_specialist_assignments() {
        assert!(parse_routing(r#"{"assignments":[]}"#, &bundles()).is_err());
    }

    #[test]
    fn fallback_runs_every_specialist() {
        let assignments = fallback_assignments(&bundles());
        assert_eq!(assignments.len(), ReviewAgent::SPECIALISTS.len());
        assert!(assignments
            .iter()
            .all(|assignment| assignment.bundle_ids.len() == 2));
    }

    #[test]
    fn documentation_assignment_excludes_agent_instruction_only_bundle() {
        let mut candidates = bundles();
        candidates.push(ReviewBundle {
            id: "instructions".to_owned(),
            paths: vec!["nested/AGENTS.md".to_owned()],
            hunk_count: 1,
            risk: RiskLevel::Low,
            related_files: Vec::new(),
        });
        let raw = r#"{"assignments":[{"agent":"documentation","bundle_ids":["instructions"],"rationale":"instructions changed"}]}"#;
        assert!(parse_routing(raw, &candidates).is_err());
    }

    #[tokio::test]
    async fn routing_timeout_fails_open_to_every_specialist() {
        let budget = Arc::new(Budget::new(0, 10_000, 1.0));
        let client = LlmClient::new("key", Some("http://127.0.0.1:1/chat".to_owned()), budget, 1)
            .expect("client");
        let bundles = bundles();
        let decision = route(
            &client,
            &ReviewManifest::default(),
            &bundles,
            &ReviewConfig::default(),
        )
        .await;
        assert!(decision.fallback);
        assert_eq!(decision.assignments.len(), ReviewAgent::SPECIALISTS.len());
    }
}
