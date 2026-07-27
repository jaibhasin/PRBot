use crate::repository::GitRepository;
use crate::types::{RelatedFile, ReviewBundle, ReviewManifest, RiskLevel};
use anyhow::Result;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::Path;

const MAX_RELATED_PER_FILE: usize = 12;

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

fn paths_from_grep(sha: &str, matches: &str) -> Vec<String> {
    matches
        .lines()
        .filter_map(|line| line.strip_prefix(&format!("{sha}:")))
        .filter_map(|line| line.split(':').next())
        .map(str::to_owned)
        .collect()
}

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

fn import_matches_path(import: &str, path: &str) -> bool {
    let normalized = import.replace("::", "/").replace('.', "/");
    let stem = Path::new(path)
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or_default();
    !stem.is_empty() && normalized.contains(stem)
}

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

fn truncate(value: &str, max_chars: usize) -> String {
    value.chars().take(max_chars).collect()
}
