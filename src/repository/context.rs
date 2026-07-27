use crate::repository::GitRepository;
use crate::types::{RelatedFile, ReviewBundle, ReviewManifest, RiskLevel};
use anyhow::Result;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::Path;

const MAX_RELATED_PER_FILE: usize = 12;

/// Populates the review manifest with related files and grouped review bundles discovered from the repository.
///
/// # Errors
///
/// Returns an error if the repository tree cannot be listed.
///
/// # Examples
///
/// ```no_run
/// # fn main() -> anyhow::Result<()> {
/// # let repository = unimplemented!();
/// # let mut manifest = unimplemented!();
/// build_context(&repository, &mut manifest)?;
/// # Ok(())
/// # }
/// ```
pub fn build_context(repository: &GitRepository, manifest: &mut ReviewManifest) -> Result<()> {
    let mut tree = repository.list_tree("head")?;
    if tree.len() > 100_000 {
        eprintln!(
            "repository tree has {} paths; limiting context discovery to 100000",
            tree.len()
        );
        tree.truncate(100_000);
    }
    let changed_paths = manifest
        .files
        .iter()
        .map(|file| file.path.clone())
        .collect::<BTreeSet<_>>();
    let symbol_search_limit = if manifest.files.len() > 100 { 3 } else { 12 };
    let mut related_by_path = BTreeMap::new();

    for file in &manifest.files {
        let revision = if matches!(file.status, crate::types::FileStatus::Deleted) {
            "base"
        } else {
            "head"
        };
        let source = repository
            .read_file(revision, &file.path, 250_000)
            .unwrap_or_default();
        let signals = source_signals(&file.path, &source);
        let mut scores: HashMap<String, (u32, BTreeSet<String>)> = HashMap::new();
        let directory = Path::new(&file.path).parent().and_then(Path::to_str);
        let stem = normalized_stem(&file.path);

        for candidate in &tree {
            if candidate == &file.path || changed_paths.contains(candidate) {
                continue;
            }
            if Path::new(candidate).parent().and_then(Path::to_str) == directory {
                add_score(&mut scores, candidate, 2, "same directory");
            }
            if normalized_stem(candidate) == stem {
                add_score(&mut scores, candidate, 8, "matching implementation or test");
            }
            if signals
                .imports
                .iter()
                .any(|import| import_matches_path(import, candidate))
            {
                add_score(&mut scores, candidate, 10, "import dependency");
            }
        }

        for symbol in signals.symbols.iter().take(symbol_search_limit) {
            if let Ok(matches) = repository.search("head", symbol, 100) {
                for path in paths_from_grep(repository.head_sha(), &matches) {
                    if path != file.path && !changed_paths.contains(&path) {
                        add_score(
                            &mut scores,
                            &path,
                            3,
                            &format!("references changed symbol {symbol}"),
                        );
                    }
                }
            }
        }

        let mut first_hop = scores
            .iter()
            .map(|(path, (score, _))| (path.clone(), *score))
            .collect::<Vec<_>>();
        first_hop.sort_by_key(|(_, score)| std::cmp::Reverse(*score));
        for (neighbor, _) in first_hop.into_iter().take(5) {
            let neighbor_source = repository
                .read_file("head", &neighbor, 100_000)
                .unwrap_or_default();
            let neighbor_signals = source_signals(&neighbor, &neighbor_source);
            for candidate in &tree {
                if candidate == &file.path
                    || candidate == &neighbor
                    || changed_paths.contains(candidate)
                {
                    continue;
                }
                if neighbor_signals
                    .imports
                    .iter()
                    .any(|import| import_matches_path(import, candidate))
                {
                    add_score(
                        &mut scores,
                        candidate,
                        4,
                        &format!("two-hop import through {neighbor}"),
                    );
                }
            }
        }

        let mut related = scores
            .into_iter()
            .map(|(path, (score, reasons))| RelatedFile {
                path,
                score,
                reasons: reasons.into_iter().collect(),
            })
            .collect::<Vec<_>>();
        related.sort_by(|left, right| {
            right
                .score
                .cmp(&left.score)
                .then_with(|| left.path.cmp(&right.path))
        });
        related.truncate(MAX_RELATED_PER_FILE);
        related_by_path.insert(file.path.clone(), related);
    }

    manifest.related_files = related_by_path;
    manifest.bundles = build_bundles(manifest);
    Ok(())
}

/// Renders a concise text summary of changed files and their related files.
///
/// The summary includes up to eight related files per changed file and is limited to
/// 16,000 Unicode characters.
///
/// # Examples
///
/// ```
/// let manifest = ReviewManifest::default();
/// assert_eq!(render_repo_map(&manifest), "");
/// ```
pub fn render_repo_map(manifest: &ReviewManifest) -> String {
    let mut output = String::new();
    for file in &manifest.files {
        output.push_str(&format!("- changed: {} ({:?})\n", file.path, file.status));
        if let Some(related) = manifest.related_files.get(&file.path) {
            for item in related.iter().take(8) {
                output.push_str(&format!(
                    "  - related: {} score={} [{}]\n",
                    item.path,
                    item.score,
                    item.reasons.join(", ")
                ));
            }
        }
    }
    truncate(&output, 16_000)
}

/// Groups changed files into review bundles with aggregated risk and related-file information.
///
/// Files are grouped by parent directory and normalized filename stem. Each bundle includes
/// its changed paths, total hunk count, highest file risk, and up to the configured number of
/// highest-scoring related files.
///
/// # Examples
///
/// ```
/// let manifest = ReviewManifest::default();
/// let bundles = build_bundles(&manifest);
///
/// assert!(bundles.is_empty());
/// ```
fn build_bundles(manifest: &ReviewManifest) -> Vec<ReviewBundle> {
    let mut groups: BTreeMap<String, Vec<&crate::types::ChangedFile>> = BTreeMap::new();
    for file in &manifest.files {
        let parent = Path::new(&file.path)
            .parent()
            .and_then(Path::to_str)
            .unwrap_or_default();
        groups
            .entry(format!("{parent}/{}", normalized_stem(&file.path)))
            .or_default()
            .push(file);
    }

    groups
        .into_iter()
        .enumerate()
        .map(|(index, (_, files))| {
            let paths = files
                .iter()
                .map(|file| file.path.clone())
                .collect::<Vec<_>>();
            let hunk_count = files.iter().map(|file| file.hunks.len()).sum();
            let risk = files
                .iter()
                .map(|file| risk_for(&file.path, &file.patch))
                .max()
                .unwrap_or(RiskLevel::Low);
            let mut related = BTreeMap::new();
            for path in &paths {
                for item in manifest.related_files.get(path).into_iter().flatten() {
                    related
                        .entry(item.path.clone())
                        .and_modify(|existing: &mut RelatedFile| {
                            existing.score = existing.score.max(item.score);
                            for reason in &item.reasons {
                                if !existing.reasons.contains(reason) {
                                    existing.reasons.push(reason.clone());
                                }
                            }
                        })
                        .or_insert_with(|| item.clone());
                }
            }
            let mut related = related.into_values().collect::<Vec<_>>();
            related.sort_by_key(|item| std::cmp::Reverse(item.score));
            related.truncate(MAX_RELATED_PER_FILE);
            ReviewBundle {
                id: format!("bundle-{}", index + 1),
                paths,
                hunk_count,
                risk,
                related_files: related,
            }
        })
        .collect()
}

/// Classifies the risk level of a file from its path and patch content.
///
/// # Examples
///
/// ```
/// assert!(matches!(risk_for("src/auth.rs", ""), RiskLevel::Critical));
/// assert!(matches!(risk_for("src/util.rs", ""), RiskLevel::Low));
/// ```
fn risk_for(path: &str, patch: &str) -> RiskLevel {
    let text = format!(
        "{} {}",
        path.to_ascii_lowercase(),
        patch.to_ascii_lowercase()
    );
    if ["auth", "permission", "secret", "token", "crypto", "payment"]
        .iter()
        .any(|signal| text.contains(signal))
    {
        RiskLevel::Critical
    } else if ["unsafe", "transaction", "migration", "concurrent", "lock"]
        .iter()
        .any(|signal| text.contains(signal))
    {
        RiskLevel::High
    } else if ["api", "schema", "config", "error", "retry"]
        .iter()
        .any(|signal| text.contains(signal))
    {
        RiskLevel::Medium
    } else {
        RiskLevel::Low
    }
}

struct SourceSignals {
    symbols: Vec<String>,
    imports: Vec<String>,
}

/// Extracts import statements and symbol names from source code.
///
/// Import signals are collected from up to 100 matching lines. Symbol signals
/// come from definitions when available, or from general symbol extraction
/// otherwise.
///
/// # Examples
///
/// ```
/// let signals = source_signals("lib.rs", "use crate::module;\nfn example() {}");
///
/// assert_eq!(signals.imports, vec!["use crate::module;"]);
/// ```
///
/// # Parameters
///
/// * `path` - Source file path used to select the appropriate syntax rules.
/// * `source` - Source code to analyze.
fn source_signals(path: &str, source: &str) -> SourceSignals {
    let imports = source
        .lines()
        .filter(|line| {
            let trimmed = line.trim_start();
            trimmed.starts_with("use ")
                || trimmed.starts_with("import ")
                || trimmed.starts_with("from ")
                || trimmed.contains("require(")
        })
        .take(100)
        .map(str::to_owned)
        .collect();
    let symbols = {
        let definitions = super::syntax::definitions_for(path, source);
        if definitions.is_empty() {
            super::syntax::symbols_for(path, source)
        } else {
            definitions
        }
    };
    SourceSignals { symbols, imports }
}

/// Extracts file paths from grep output prefixed with the specified commit SHA.
///
/// # Examples
///
/// ```
/// let paths = paths_from_grep("abc123", "abc123:src/lib.rs:10:match\nother:src/main.rs:5:match");
/// assert_eq!(paths, vec!["src/lib.rs"]);
/// ```
fn paths_from_grep(sha: &str, matches: &str) -> Vec<String> {
    matches
        .lines()
        .filter_map(|line| line.strip_prefix(&format!("{sha}:")))
        .filter_map(|line| line.split(':').next())
        .map(str::to_owned)
        .collect()
}

/// Produces a normalized file stem by removing test-related suffixes.
///
/// # Examples
///
/// ```
/// assert_eq!(normalized_stem("src/parser_test.rs"), "parser");
/// assert_eq!(normalized_stem("src/parser.spec.ts"), "parser");
/// ```
fn normalized_stem(path: &str) -> String {
    let stem = Path::new(path)
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or(path);
    stem.trim_end_matches("_test")
        .trim_end_matches(".test")
        .trim_end_matches(".spec")
        .to_owned()
}

/// Determines whether an import reference contains a file's stem.
///
/// # Examples
///
/// ```
/// assert!(import_matches_path("crate::utils::parser", "src/parser.rs"));
/// assert!(!import_matches_path("crate::utils::parser", "src/reader.rs"));
/// ```
fn import_matches_path(import: &str, path: &str) -> bool {
    let normalized = import.replace("::", "/").replace('.', "/");
    let stem = Path::new(path)
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or_default();
    !stem.is_empty() && normalized.contains(stem)
}

/// Adds a weighted reason to a path's accumulated score.
///
/// # Examples
///
/// ```
/// use std::collections::{BTreeSet, HashMap};
///
/// let mut scores: HashMap<String, (u32, BTreeSet<String>)> = HashMap::new();
/// add_score(&mut scores, "src/lib.rs", 3, "same directory");
///
/// assert_eq!(scores["src/lib.rs"].0, 3);
/// assert!(scores["src/lib.rs"].1.contains("same directory"));
/// ```
fn add_score(
    scores: &mut HashMap<String, (u32, BTreeSet<String>)>,
    path: &str,
    score: u32,
    reason: &str,
) {
    let entry = scores.entry(path.to_owned()).or_default();
    entry.0 += score;
    entry.1.insert(reason.to_owned());
}

/// Limits a string to a maximum number of Unicode characters.
///
/// # Examples
///
/// ```
/// assert_eq!(truncate("hello", 3), "hel");
/// assert_eq!(truncate("café", 4), "café");
/// ```
///
/// The resulting string contains at most `max_chars` Unicode characters.
fn truncate(value: &str, max_chars: usize) -> String {
    value.chars().take(max_chars).collect()
}
