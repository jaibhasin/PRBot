use crate::config::ReviewConfig;
use crate::types::{ReviewBundle, RiskLevel};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DepthPlan {
    pub primary_passes: usize,
    pub primary_max_steps: usize,
    pub verifier_max_steps: usize,
}

/// Returns the highest risk level among selected bundles.
pub fn max_bundle_risk(bundles: &[ReviewBundle]) -> RiskLevel {
    bundles
        .iter()
        .map(|bundle| bundle.risk)
        .max()
        .unwrap_or(RiskLevel::Low)
}

/// Chooses pass count and tool-step budgets from risk, clamped by config ceilings.
pub fn depth_for(risk: RiskLevel, config: &ReviewConfig) -> DepthPlan {
    let (passes, primary_steps, verifier_steps) = match risk {
        RiskLevel::Low => (1, 4, 4),
        RiskLevel::Medium => (1, 6, 6),
        RiskLevel::High => (2, 8, 6),
        RiskLevel::Critical => (3, 10, 8),
    };
    DepthPlan {
        primary_passes: passes.min(config.primary_passes.max(1)).clamp(1, 3),
        primary_max_steps: primary_steps.min(config.primary_max_steps.max(1)),
        verifier_max_steps: verifier_steps.min(config.verifier_max_steps.max(1)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::RiskLevel;

    fn bundle(risk: RiskLevel) -> ReviewBundle {
        ReviewBundle {
            id: "b".to_owned(),
            paths: vec!["src/a.rs".to_owned()],
            hunk_count: 1,
            risk,
            related_files: Vec::new(),
        }
    }

    #[test]
    fn max_risk_uses_highest_bundle() {
        let bundles = vec![bundle(RiskLevel::Low), bundle(RiskLevel::Critical)];
        assert_eq!(max_bundle_risk(&bundles), RiskLevel::Critical);
    }

    #[test]
    fn depth_clamps_to_config_ceiling() {
        let config = ReviewConfig {
            primary_passes: 1,
            ..ReviewConfig::default()
        };
        let plan = depth_for(RiskLevel::Critical, &config);
        assert_eq!(plan.primary_passes, 1);
        assert_eq!(plan.primary_max_steps, 10);
    }

    #[test]
    fn high_risk_can_use_two_passes() {
        let config = ReviewConfig {
            primary_passes: 3,
            ..ReviewConfig::default()
        };
        let plan = depth_for(RiskLevel::High, &config);
        assert_eq!(plan.primary_passes, 2);
        assert_eq!(plan.primary_max_steps, 8);
    }
}
