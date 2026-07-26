use anyhow::{bail, Context, Result};
use globset::{Glob, GlobSet, GlobSetBuilder};
use serde::Deserialize;

pub const DEFAULT_REVIEW_MODEL: &str = "deepseek/deepseek-v4-pro";
pub const DEFAULT_VERIFICATION_MODEL: &str = "openai/gpt-5.6-luna";

#[derive(Clone, Debug)]
pub struct ReviewConfig {
    pub review_model: String,
    pub verification_model: String,
    pub max_review_minutes: u64,
    pub max_input_tokens: u64,
    pub max_cost_usd: f64,
    pub max_concurrency: usize,
    pub max_comments: usize,
    pub engine: ReviewEngine,
    pub auto_review_owner_authored: bool,
    pub include: Vec<String>,
    pub exclude: Vec<String>,
    pub instructions: Vec<String>,
    pub path_rules: Vec<PathRule>,
}

impl Default for ReviewConfig {
    fn default() -> Self {
        Self {
            review_model: DEFAULT_REVIEW_MODEL.to_owned(),
            verification_model: DEFAULT_VERIFICATION_MODEL.to_owned(),
            max_review_minutes: 15,
            max_input_tokens: 500_000,
            max_cost_usd: 3.0,
            max_concurrency: 8,
            max_comments: 12,
            engine: ReviewEngine::Legacy,
            auto_review_owner_authored: true,
            include: vec!["**/*".to_owned()],
            exclude: default_excludes(),
            instructions: Vec::new(),
            path_rules: Vec::new(),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReviewEngine {
    Contextual,
    Legacy,
}

impl ReviewEngine {
    pub fn parse(value: &str) -> Result<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "contextual" => Ok(Self::Contextual),
            "legacy" => Ok(Self::Legacy),
            other => bail!("invalid review engine '{other}', expected contextual or legacy"),
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct PathRule {
    pub glob: String,
    #[serde(default)]
    pub instructions: Vec<String>,
}

#[derive(Debug, Default, Deserialize)]
struct RepositoryConfig {
    #[serde(default)]
    review: RepositoryReviewConfig,
    #[serde(default)]
    path_rules: Vec<PathRule>,
}

#[derive(Debug, Default, Deserialize)]
struct RepositoryReviewConfig {
    auto_review: Option<String>,
    include: Option<Vec<String>>,
    exclude: Option<Vec<String>>,
    instructions: Option<Vec<String>>,
    max_comments: Option<usize>,
}

impl ReviewConfig {
    pub fn apply_repository_toml(&mut self, source: &str) -> Result<()> {
        let parsed: RepositoryConfig =
            toml::from_str(source).context("failed to parse trusted .prbot.toml")?;
        if let Some(policy) = parsed.review.auto_review {
            if policy != "owner-authored" && policy != "off" {
                bail!("invalid auto_review policy '{policy}', expected owner-authored or off");
            }
            self.auto_review_owner_authored = policy == "owner-authored";
        }
        if let Some(include) = parsed.review.include {
            validate_globs(&include)?;
            self.include = include;
        }
        if let Some(exclude) = parsed.review.exclude {
            validate_globs(&exclude)?;
            self.exclude = exclude;
        }
        if let Some(instructions) = parsed.review.instructions {
            self.instructions = instructions;
        }
        if let Some(max_comments) = parsed.review.max_comments {
            self.max_comments = self.max_comments.min(max_comments);
        }
        for rule in &parsed.path_rules {
            Glob::new(&rule.glob)
                .with_context(|| format!("invalid path rule glob '{}'", rule.glob))?;
        }
        self.path_rules = parsed.path_rules;
        Ok(())
    }

    pub fn path_filter(&self) -> Result<PathFilter> {
        Ok(PathFilter {
            include: build_globset(&self.include)?,
            exclude: build_globset(&self.exclude)?,
        })
    }

    pub fn instructions_for(&self, path: &str) -> Vec<String> {
        let mut result = self.instructions.clone();
        for rule in &self.path_rules {
            if Glob::new(&rule.glob)
                .ok()
                .map(|glob| glob.compile_matcher().is_match(path))
                .unwrap_or(false)
            {
                result.extend(rule.instructions.clone());
            }
        }
        result
    }
}

pub struct PathFilter {
    include: GlobSet,
    exclude: GlobSet,
}

impl PathFilter {
    pub fn is_reviewable(&self, path: &str) -> bool {
        self.include.is_match(path) && !self.exclude.is_match(path) && supported_file(path)
    }
}

fn build_globset(patterns: &[String]) -> Result<GlobSet> {
    let mut builder = GlobSetBuilder::new();
    for pattern in patterns {
        builder.add(Glob::new(pattern).with_context(|| format!("invalid glob '{pattern}'"))?);
    }
    builder
        .build()
        .context("failed to compile glob configuration")
}

fn validate_globs(patterns: &[String]) -> Result<()> {
    build_globset(patterns).map(|_| ())
}

fn supported_file(path: &str) -> bool {
    let extension = path.rsplit('.').next().unwrap_or_default();
    matches!(
        extension,
        "rs" | "py"
            | "js"
            | "jsx"
            | "ts"
            | "tsx"
            | "go"
            | "java"
            | "kt"
            | "kts"
            | "rb"
            | "php"
            | "cs"
            | "cpp"
            | "cc"
            | "c"
            | "h"
            | "hpp"
            | "swift"
            | "scala"
            | "sh"
            | "sql"
            | "yaml"
            | "yml"
            | "toml"
            | "css"
            | "scss"
            | "sass"
            | "less"
    )
}

fn default_excludes() -> Vec<String> {
    [
        "**/vendor/**",
        "**/generated/**",
        "**/node_modules/**",
        "**/*.min.js",
        "**/*.lock",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repository_config_can_only_reduce_comment_ceiling() {
        let mut config = ReviewConfig {
            max_comments: 8,
            ..ReviewConfig::default()
        };
        config
            .apply_repository_toml("[review]\nmax_comments = 20\n")
            .expect("config");
        assert_eq!(config.max_comments, 8);
    }

    #[test]
    fn filters_generated_and_unsupported_files() {
        let filter = ReviewConfig::default().path_filter().expect("filter");
        assert!(filter.is_reviewable("src/main.rs"));
        assert!(!filter.is_reviewable("vendor/main.rs"));
        assert!(!filter.is_reviewable("assets/logo.png"));
    }

    #[test]
    fn trusted_config_can_disable_automatic_reviews() {
        let mut config = ReviewConfig::default();
        config
            .apply_repository_toml("[review]\nauto_review = \"off\"\n")
            .expect("config");
        assert!(!config.auto_review_owner_authored);
    }

    #[test]
    fn rejects_invalid_policy_and_glob() {
        let mut config = ReviewConfig::default();
        assert!(config
            .apply_repository_toml("[review]\nauto_review = \"everyone\"\n")
            .is_err());
        assert!(config
            .apply_repository_toml("[review]\ninclude = [\"[\"]\n")
            .is_err());
    }
}
