use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;

#[derive(Clone, Debug, Default, Serialize)]
pub struct ReviewManifest {
    pub files: Vec<ChangedFile>,
    pub bundles: Vec<ReviewBundle>,
    pub ignored: Vec<IgnoredFile>,
    pub related_files: BTreeMap<String, Vec<RelatedFile>>,
}

impl ReviewManifest {
    /// Counts the diff hunks eligible for review.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::collections::BTreeMap;
    ///
    /// let manifest = ReviewManifest {
    ///     files: Vec::new(),
    ///     bundles: Vec::new(),
    ///     ignored: Vec::new(),
    ///     related_files: BTreeMap::new(),
    /// };
    ///
    /// assert_eq!(manifest.eligible_hunks(), 0);
    /// ```
    pub fn eligible_hunks(&self) -> usize {
        self.files.iter().map(|file| file.hunks.len()).sum()
    }

    /// Calculates the total number of hunks assigned to review bundles.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::collections::BTreeMap;
    ///
    /// let manifest = ReviewManifest {
    ///     files: vec![],
    ///     bundles: vec![
    ///         ReviewBundle {
    ///             id: "bundle-1".into(),
    ///             paths: vec![],
    ///             hunk_count: 2,
    ///             risk: RiskLevel::Low,
    ///             related_files: vec![],
    ///         },
    ///         ReviewBundle {
    ///             id: "bundle-2".into(),
    ///             paths: vec![],
    ///             hunk_count: 3,
    ///             risk: RiskLevel::Medium,
    ///             related_files: vec![],
    ///         },
    ///     ],
    ///     ignored: vec![],
    ///     related_files: BTreeMap::new(),
    /// };
    ///
    /// assert_eq!(manifest.assigned_hunks(), 5);
    /// ```
    pub fn assigned_hunks(&self) -> usize {
        self.bundles.iter().map(|bundle| bundle.hunk_count).sum()
    }

    /// Determines whether all eligible hunks have been assigned to review bundles.
    ///
    /// # Examples
    ///
    /// ```
    /// let manifest = ReviewManifest {
    ///     files: vec![],
    ///     bundles: vec![],
    ///     ignored: vec![],
    ///     related_files: std::collections::BTreeMap::new(),
    /// };
    ///
    /// assert!(manifest.complete());
    /// ```
    pub fn complete(&self) -> bool {
        self.eligible_hunks() == self.assigned_hunks()
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct ChangedFile {
    pub path: String,
    pub old_path: Option<String>,
    pub status: FileStatus,
    pub patch: String,
    pub hunks: Vec<DiffHunk>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FileStatus {
    Added,
    Modified,
    Deleted,
    Renamed,
    Copied,
    Unknown,
}

#[derive(Clone, Debug, Serialize)]
pub struct DiffHunk {
    pub header: String,
    pub old_start: u64,
    pub new_start: u64,
    pub lines: Vec<DiffLine>,
}

#[derive(Clone, Debug, Serialize)]
pub struct DiffLine {
    pub side: DiffSide,
    pub old_line: Option<u64>,
    pub new_line: Option<u64>,
    pub content: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum DiffSide {
    Left,
    Right,
    Context,
}

#[derive(Clone, Debug, Serialize)]
pub struct IgnoredFile {
    pub path: String,
    pub reason: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct RelatedFile {
    pub path: String,
    pub score: u32,
    pub reasons: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct ReviewBundle {
    pub id: String,
    pub paths: Vec<String>,
    pub hunk_count: usize,
    pub risk: RiskLevel,
    pub related_files: Vec<RelatedFile>,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RiskLevel {
    Low,
    Medium,
    High,
    Critical,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct CandidateFinding {
    #[serde(default)]
    pub agent: ReviewAgent,
    pub path: String,
    pub side: DiffSide,
    pub anchor: String,
    #[serde(default)]
    pub end_anchor: Option<String>,
    pub priority: Priority,
    pub category: FindingCategory,
    pub title: String,
    pub body: String,
    #[serde(default)]
    pub evidence: Vec<EvidenceSpan>,
    #[serde(default)]
    pub confidence: f32,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
pub enum Priority {
    P0,
    P1,
    P2,
    P3,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FindingCategory {
    Correctness,
    Architecture,
    Security,
    Reliability,
    Compatibility,
    Performance,
    Concurrency,
    Api,
    Documentation,
    Other,
}

#[derive(
    Clone, Copy, Debug, Default, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize,
)]
#[serde(rename_all = "snake_case")]
pub enum ReviewAgent {
    #[default]
    Primary,
}

impl ReviewAgent {
    pub fn title(self) -> &'static str {
        "Precision review"
    }
}

impl fmt::Display for ReviewAgent {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("primary")
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentStatus {
    Skipped,
    Completed,
    Failed,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct AgentRun {
    pub agent: ReviewAgent,
    pub status: AgentStatus,
    pub bundle_ids: Vec<String>,
    pub rationale: String,
    pub candidate_findings: usize,
    pub accepted_findings: usize,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct EvidenceSpan {
    pub path: String,
    pub revision: Revision,
    #[serde(default)]
    pub start_line: Option<u64>,
    #[serde(default)]
    pub end_line: Option<u64>,
    pub explanation: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Revision {
    Base,
    Head,
}

#[derive(Clone, Debug, Serialize)]
pub struct ResolvedFinding {
    pub candidate: CandidateFinding,
    pub line: Option<u64>,
    pub start_line: Option<u64>,
    pub side: DiffSide,
    pub fingerprint: String,
    #[serde(default)]
    pub file_level: bool,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct BudgetSnapshot {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub estimated_cost_usd: f64,
    pub elapsed_seconds: u64,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    Complete,
    Partial,
    Skipped,
    Failed,
}

#[derive(Clone, Debug, Serialize)]
pub struct ReviewRun {
    pub trigger: String,
    pub actor: String,
    pub repository: String,
    pub pr_number: u64,
    pub base_sha: String,
    pub head_sha: String,
    pub previous_head_sha: Option<String>,
    pub incremental: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct RunOutcome {
    pub status: RunStatus,
    pub reviewed_sha: String,
    pub coverage_complete: bool,
    pub eligible_hunks: usize,
    pub assigned_hunks: usize,
    pub findings: usize,
    #[serde(default)]
    pub active_findings: usize,
    #[serde(default)]
    pub ever_published_findings: usize,
    #[serde(default)]
    pub resolved_findings: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolution_rate: Option<f64>,
    pub skipped_findings: usize,
    pub failed_bundles: Vec<String>,
    pub budget: BudgetSnapshot,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub incremental: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reviewed_bundles: Option<usize>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub agent_runs: Vec<AgentRun>,
}
