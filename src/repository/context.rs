use crate::repository::GitRepository;
use crate::types::{RelatedFile, ReviewBundle, ReviewManifest, RiskLevel};
use anyhow::Result;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::Path;
use tree_sitter::{Language, Node, Parser};

const MAX_SYMBOLS_PER_FILE: usize = 24;
const MAX_RELATED_PER_FILE: usize = 12;

pub fn build_context(repository: &GitRepository, manifest: &mut ReviewManifest) -> Result<()> {
    let tree = repository.list_tree("head")?;
    let changed_paths = manifest
        .files
        .iter()
        .map(|file| file.path.clone())
        .collect::<BTreeSet<_>>();
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

        for symbol in signals.symbols.iter().take(12) {
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
    let symbols = language_for(path)
        .and_then(|language| syntax_symbols(language, source))
        .unwrap_or_else(|| lexical_symbols(source));
    SourceSignals { symbols, imports }
}

fn syntax_symbols(language: Language, source: &str) -> Option<Vec<String>> {
    let mut parser = Parser::new();
    parser.set_language(&language).ok()?;
    let tree = parser.parse(source, None)?;
    let mut symbols = BTreeSet::new();
    collect_identifiers(tree.root_node(), source.as_bytes(), &mut symbols);
    Some(symbols.into_iter().take(MAX_SYMBOLS_PER_FILE).collect())
}

fn collect_identifiers(node: Node<'_>, source: &[u8], output: &mut BTreeSet<String>) {
    if output.len() >= MAX_SYMBOLS_PER_FILE {
        return;
    }
    let kind = node.kind();
    if (kind == "identifier" || kind.ends_with("_identifier"))
        && node.child_count() == 0
        && node.end_byte().saturating_sub(node.start_byte()) <= 80
    {
        if let Ok(value) = node.utf8_text(source) {
            if value.len() > 2 {
                output.insert(value.to_owned());
            }
        }
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_identifiers(child, source, output);
    }
}

fn lexical_symbols(source: &str) -> Vec<String> {
    source
        .split(|character: char| !character.is_alphanumeric() && character != '_')
        .filter(|value| value.len() > 3)
        .map(str::to_owned)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .take(MAX_SYMBOLS_PER_FILE)
        .collect()
}

fn language_for(path: &str) -> Option<Language> {
    match path.rsplit('.').next()? {
        "rs" => Some(tree_sitter_rust::LANGUAGE.into()),
        "py" => Some(tree_sitter_python::LANGUAGE.into()),
        "go" => Some(tree_sitter_go::LANGUAGE.into()),
        "js" | "jsx" => Some(tree_sitter_javascript::LANGUAGE.into()),
        "ts" => Some(tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into()),
        "tsx" => Some(tree_sitter_typescript::LANGUAGE_TSX.into()),
        _ => None,
    }
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
