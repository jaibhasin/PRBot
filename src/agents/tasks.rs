use super::router::RoutingAssignment;
use crate::types::{AgentRun, AgentStatus, ReviewAgent, ReviewBundle};
use std::collections::BTreeMap;

pub(super) fn initial_agent_runs(
    bundles: &[ReviewBundle],
    assignments: &[RoutingAssignment],
) -> BTreeMap<ReviewAgent, AgentRun> {
    let all_bundle_ids = bundles
        .iter()
        .map(|bundle| bundle.id.clone())
        .collect::<Vec<_>>();
    ReviewAgent::REVIEWERS
        .into_iter()
        .map(|agent| {
            let assignment = assignments
                .iter()
                .find(|assignment| assignment.agent == agent);
            let run = if agent == ReviewAgent::Correctness {
                AgentRun {
                    agent,
                    status: AgentStatus::Completed,
                    bundle_ids: all_bundle_ids.clone(),
                    rationale: "Always-on review for every selected bundle.".to_owned(),
                    candidate_findings: 0,
                    accepted_findings: 0,
                }
            } else if let Some(assignment) = assignment {
                AgentRun {
                    agent,
                    status: AgentStatus::Completed,
                    bundle_ids: assignment.bundle_ids.clone(),
                    rationale: assignment.rationale.clone(),
                    candidate_findings: 0,
                    accepted_findings: 0,
                }
            } else {
                AgentRun {
                    agent,
                    status: AgentStatus::Skipped,
                    bundle_ids: Vec::new(),
                    rationale: "Not selected by the routing agent.".to_owned(),
                    candidate_findings: 0,
                    accepted_findings: 0,
                }
            };
            (agent, run)
        })
        .collect()
}

pub(super) fn empty_agent_runs() -> Vec<AgentRun> {
    ReviewAgent::REVIEWERS
        .into_iter()
        .map(|agent| AgentRun {
            agent,
            status: if agent == ReviewAgent::Correctness {
                AgentStatus::Completed
            } else {
                AgentStatus::Skipped
            },
            bundle_ids: Vec::new(),
            rationale: "No review bundles were selected.".to_owned(),
            candidate_findings: 0,
            accepted_findings: 0,
        })
        .collect()
}

pub(super) fn build_tasks(
    bundles: &[ReviewBundle],
    assignments: &[RoutingAssignment],
) -> Vec<ReviewTask> {
    let by_id = bundles
        .iter()
        .map(|bundle| (bundle.id.as_str(), bundle))
        .collect::<BTreeMap<_, _>>();
    let mut tasks = bundles
        .iter()
        .cloned()
        .map(|bundle| ReviewTask {
            agent: ReviewAgent::Correctness,
            label: format!("{}:correctness", bundle.id),
            bundles: vec![bundle],
        })
        .collect::<Vec<_>>();
    for assignment in assignments {
        let selected = assignment
            .bundle_ids
            .iter()
            .filter_map(|id| by_id.get(id.as_str()).map(|bundle| (*bundle).clone()))
            .collect::<Vec<_>>();
        match assignment.agent {
            ReviewAgent::Security | ReviewAgent::Performance => {
                tasks.extend(selected.into_iter().map(|bundle| ReviewTask {
                    agent: assignment.agent,
                    label: format!("{}:{}", bundle.id, assignment.agent),
                    bundles: vec![bundle],
                }));
            }
            ReviewAgent::Architecture | ReviewAgent::Documentation => {
                tasks.push(ReviewTask {
                    agent: assignment.agent,
                    label: assignment.agent.to_string(),
                    bundles: selected,
                });
            }
            ReviewAgent::Correctness => {}
        }
    }
    tasks
}

#[derive(Debug)]
pub(super) struct ReviewTask {
    pub agent: ReviewAgent,
    pub label: String,
    pub bundles: Vec<ReviewBundle>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::RiskLevel;

    fn bundle(id: &str) -> ReviewBundle {
        ReviewBundle {
            id: id.to_owned(),
            paths: vec![format!("src/{id}.rs")],
            hunk_count: 1,
            risk: RiskLevel::High,
            related_files: Vec::new(),
        }
    }

    #[test]
    fn builds_correctness_per_bundle_and_groups_pr_wide_specialists() {
        let bundles = vec![bundle("api"), bundle("config")];
        let assignments = vec![
            RoutingAssignment {
                agent: ReviewAgent::Architecture,
                bundle_ids: vec!["api".to_owned(), "config".to_owned()],
                rationale: "cross-file contract".to_owned(),
            },
            RoutingAssignment {
                agent: ReviewAgent::Security,
                bundle_ids: vec!["api".to_owned(), "config".to_owned()],
                rationale: "input boundary".to_owned(),
            },
        ];
        let tasks = build_tasks(&bundles, &assignments);
        assert_eq!(
            tasks
                .iter()
                .filter(|task| task.agent == ReviewAgent::Correctness)
                .count(),
            2
        );
        assert_eq!(
            tasks
                .iter()
                .filter(|task| task.agent == ReviewAgent::Architecture)
                .count(),
            1
        );
        assert_eq!(
            tasks
                .iter()
                .filter(|task| task.agent == ReviewAgent::Security)
                .count(),
            2
        );
    }
}
