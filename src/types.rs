use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Debug, Default, Serialize)]
pub struct ReviewManifest {
    pub files: Vec<ChangedFile>,
    pub bundles: Vec<ReviewBundle>,
    pub ignored: Vec<IgnoredFile>,
    pub related_files: BTreeMap<String, Vec<RelatedFile>>,
}

impl ReviewManifest {
    pub fn eligible_hunks(&self) -> usize {
        self.files.iter().map(|file| file.hunks.len()).sum()
    }

    pub fn assigned_hunks(&self) -> usize {
        self.bundles.iter().map(|bundle| bundle.hunk_count).sum()
    }

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
    Security,
    Reliability,
    Compatibility,
    Performance,
    Concurrency,
    Api,
    Other,
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
}

#[derive(Clone, Debug, Serialize)]
pub struct RunOutcome {
    pub status: RunStatus,
    pub reviewed_sha: String,
    pub coverage_complete: bool,
    pub eligible_hunks: usize,
    pub assigned_hunks: usize,
    pub findings: usize,
    pub skipped_findings: usize,
    pub failed_bundles: Vec<String>,
    pub budget: BudgetSnapshot,
}
