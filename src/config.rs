use anyhow::{bail, Context, Result};
use globset::{Glob, GlobSet, GlobSetBuilder};
use serde::Deserialize;

pub const DEFAULT_REVIEW_MODEL: &str = "deepseek/deepseek-v4-flash";
pub const DEFAULT_VERIFICATION_MODEL: &str = "deepseek/deepseek-v4-flash";

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum CollaboratorPermission {
    Write = 1,
    Maintain = 2,
    Admin = 3,
}

impl CollaboratorPermission {
    /// Parses a collaborator permission floor.
    ///
    /// Accepted values are `admin`, `maintain`, and `write` (case-insensitive).
    pub fn parse(value: &str) -> Result<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "admin" => Ok(Self::Admin),
            "maintain" => Ok(Self::Maintain),
            "write" => Ok(Self::Write),
            other => bail!("invalid min_permission '{other}', expected admin, maintain, or write"),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Admin => "admin",
            Self::Maintain => "maintain",
            Self::Write => "write",
        }
    }

    /// Returns whether a GitHub API permission string meets this floor.
    pub fn meets(self, actual: &str) -> bool {
        Self::from_api(actual).is_some_and(|permission| permission >= self)
    }

    fn from_api(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "admin" => Some(Self::Admin),
            "maintain" => Some(Self::Maintain),
            "write" => Some(Self::Write),
            _ => None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct ReviewConfig {
    pub review_model: String,
    pub verification_model: String,
    pub max_review_minutes: u64,
    pub max_input_tokens: u64,
    pub max_cost_usd: f64,
    pub max_concurrency: usize,
    pub max_comments: usize,
    pub primary_passes: usize,
    pub primary_max_steps: usize,
    pub verifier_max_steps: usize,
    pub majority_k: usize,
    pub keep_high_confidence_singleton: f32,
    pub enable_walkthrough: bool,
    pub min_permission: CollaboratorPermission,
    pub engine: ReviewEngine,
    pub auto_review_owner_authored: bool,
    pub include: Vec<String>,
    pub exclude: Vec<String>,
    pub instructions: Vec<String>,
    pub path_rules: Vec<PathRule>,
}

impl Default for ReviewConfig {
    /// Creates a review configuration with the default models, limits, filters, and review policies.
    ///
    /// # Examples
    ///
    /// ```
    /// let config = ReviewConfig::default();
    /// assert_eq!(config.max_comments, 12);
    /// assert!(config.auto_review_owner_authored);
    /// ```
    fn default() -> Self {
        Self {
            review_model: DEFAULT_REVIEW_MODEL.to_owned(),
            verification_model: DEFAULT_VERIFICATION_MODEL.to_owned(),
            max_review_minutes: 15,
            max_input_tokens: 500_000,
            max_cost_usd: 3.0,
            max_concurrency: 8,
            max_comments: 12,
            primary_passes: 1,
            primary_max_steps: 10,
            verifier_max_steps: 8,
            majority_k: 2,
            keep_high_confidence_singleton: 0.92,
            enable_walkthrough: true,
            min_permission: CollaboratorPermission::Admin,
            engine: ReviewEngine::Contextual,
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
    /// Parses a review engine name.
    ///
    /// Input is trimmed and matched case-insensitively against `contextual` and
    /// `legacy`.
    ///
    /// # Examples
    ///
    /// ```
    /// assert!(matches!(ReviewEngine::parse(" Contextual "), Ok(ReviewEngine::Contextual)));
    /// ```
    ///
    /// # Errors
    ///
    /// Returns an error when the value is not `contextual` or `legacy`.
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
    min_permission: Option<String>,
    primary_passes: Option<usize>,
}

impl ReviewConfig {
    /// Applies trusted repository review settings from TOML.
    ///
    /// Repository settings may update review policies, path filters, instructions, comment limits,
    /// and path-specific rules. The configured comment limit can only be reduced.
    ///
    /// # Errors
    ///
    /// Returns an error if the TOML is invalid, an auto-review policy is unsupported, or any
    /// configured glob is invalid.
    ///
    /// # Examples
    ///
    /// ```
    /// let mut config = ReviewConfig::default();
    /// config
    ///     .apply_repository_toml("[review]\nmax_comments = 5")
    ///     .unwrap();
    /// assert_eq!(config.max_comments, 5);
    /// ```
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
        if let Some(min_permission) = parsed.review.min_permission {
            self.min_permission = CollaboratorPermission::parse(&min_permission)?;
        }
        if let Some(primary_passes) = parsed.review.primary_passes {
            if primary_passes == 0 || primary_passes > 3 {
                bail!("invalid primary_passes '{primary_passes}', expected 1..=3");
            }
            self.primary_passes = primary_passes;
        }
        for rule in &parsed.path_rules {
            Glob::new(&rule.glob)
                .with_context(|| format!("invalid path rule glob '{}'", rule.glob))?;
        }
        self.path_rules = parsed.path_rules;
        Ok(())
    }

    /// Builds a path filter from the configured include and exclude patterns.
    ///
    /// # Examples
    ///
    /// ```
    /// let config = ReviewConfig::default();
    /// let filter = config.path_filter().unwrap();
    ///
    /// assert!(filter.is_reviewable("src/main.rs"));
    /// ```
    ///
    /// Returns an error if any configured glob pattern is invalid.
    pub fn path_filter(&self) -> Result<PathFilter> {
        Ok(PathFilter {
            include: build_globset(&self.include)?,
            exclude: build_globset(&self.exclude)?,
        })
    }

    /// Collects the base instructions and instructions from rules matching a path.
    ///
    /// Invalid path-rule globs are ignored.
    ///
    /// # Examples
    ///
    /// ```
    /// let config = ReviewConfig::default();
    /// assert!(config.instructions_for("src/main.rs").is_empty());
    /// ```
    ///
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
    /// Determines whether a path is eligible for review.
    ///
    /// A path is eligible when it matches the include patterns, does not match the
    /// exclude patterns, and uses a supported file extension.
    ///
    /// # Returns
    ///
    /// `true` if the path is eligible for review, `false` otherwise.
    ///
    /// # Examples
    ///
    /// ```
    /// let filter = ReviewConfig::default().path_filter().unwrap();
    ///
    /// assert!(filter.is_reviewable("src/main.rs"));
    /// assert!(!filter.is_reviewable("assets/logo.png"));
    /// ```
    pub fn is_reviewable(&self, path: &str) -> bool {
        self.include.is_match(path) && !self.exclude.is_match(path) && supported_file(path)
    }
}

/// Compiles glob patterns into a matcher.
///
/// # Errors
///
/// Returns an error if any pattern is invalid or the matcher cannot be built.
///
/// # Examples
///
/// ```
/// let globs = build_globset(&["src/**/*.rs".to_string()])?;
/// assert!(globs.is_match("src/main.rs"));
/// # Ok::<(), anyhow::Error>(())
/// ```
///
/// # Returns
///
/// The compiled glob matcher.
fn build_globset(patterns: &[String]) -> Result<GlobSet> {
    let mut builder = GlobSetBuilder::new();
    for pattern in patterns {
        builder.add(Glob::new(pattern).with_context(|| format!("invalid glob '{pattern}'"))?);
    }
    builder
        .build()
        .context("failed to compile glob configuration")
}

/// Validates a collection of glob patterns.
///
/// # Examples
///
/// ```
/// let patterns = vec!["src/**/*.rs".to_string()];
/// assert!(validate_globs(&patterns).is_ok());
/// ```
fn validate_globs(patterns: &[String]) -> Result<()> {
    build_globset(patterns).map(|_| ())
}

/// Determines whether a path has a supported source or configuration file extension.
///
/// # Examples
///
/// ```
/// assert!(supported_file("src/main.rs"));
/// assert!(!supported_file("assets/logo.png"));
/// ```
///
/// # Returns
///
/// `true` if the path ends with a supported extension, `false` otherwise.
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

/// Provides glob patterns for commonly excluded paths and generated files.
///
/// # Examples
///
/// ```
/// let excludes = default_excludes();
/// assert!(excludes.contains(&"**/vendor/**".to_owned()));
/// assert!(excludes.contains(&"**/*.lock".to_owned()));
/// ```
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
    fn permission_floor_accepts_higher_levels() {
        assert!(CollaboratorPermission::Write.meets("write"));
        assert!(CollaboratorPermission::Write.meets("maintain"));
        assert!(CollaboratorPermission::Write.meets("admin"));
        assert!(!CollaboratorPermission::Write.meets("read"));
        assert!(!CollaboratorPermission::Admin.meets("write"));
        assert!(CollaboratorPermission::parse("WRITE").is_ok());
        assert!(CollaboratorPermission::parse("triage").is_err());
    }

    #[test]
    fn repository_config_can_set_permission_and_passes() {
        let mut config = ReviewConfig::default();
        config
            .apply_repository_toml("[review]\nmin_permission = \"write\"\nprimary_passes = 2\n")
            .expect("config");
        assert_eq!(config.min_permission, CollaboratorPermission::Write);
        assert_eq!(config.primary_passes, 2);
    }

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
