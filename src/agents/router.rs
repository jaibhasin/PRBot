use super::parse_json;
use super::prompts;
use crate::config::ReviewConfig;
use crate::llm::LlmClient;
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
    let valid_ids = bundles
        .iter()
        .map(|bundle| bundle.id.as_str())
        .collect::<BTreeSet<_>>();
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
            if !valid_ids.contains(id.as_str()) {
                bail!("router returned unknown bundle '{id}'");
            }
            entry.0.insert(id);
        }
        entry.1.push(assignment.rationale.trim().to_owned());
    }
    Ok(merged
        .into_iter()
        .map(|(agent, (bundle_ids, rationales))| RoutingAssignment {
            agent,
            bundle_ids: bundle_ids.into_iter().collect(),
            rationale: rationales.join("; "),
        })
        .collect())
}

fn fallback_assignments(bundles: &[ReviewBundle]) -> Vec<RoutingAssignment> {
    let bundle_ids = bundles
        .iter()
        .map(|bundle| bundle.id.clone())
        .collect::<Vec<_>>();
    ReviewAgent::SPECIALISTS
        .into_iter()
        .map(|agent| RoutingAssignment {
            agent,
            bundle_ids: bundle_ids.clone(),
            rationale: "Router failed, so PRBot ran every specialist.".to_owned(),
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
    use crate::types::RiskLevel;

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
    fn fallback_runs_every_specialist() {
        let assignments = fallback_assignments(&bundles());
        assert_eq!(assignments.len(), ReviewAgent::SPECIALISTS.len());
        assert!(assignments
            .iter()
            .all(|assignment| assignment.bundle_ids.len() == 2));
    }
}
