use crate::types::{CandidateFinding, ChangedFile, DiffLine, DiffSide, ResolvedFinding};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;

/// Resolves candidate findings against changed files and counts candidates that could not be resolved.
///
/// # Examples
///
/// ```
/// let (resolved, skipped) = resolve_findings(vec![], &[]);
/// assert!(resolved.is_empty());
/// assert_eq!(skipped, 0);
/// ```
///
/// # Returns
///
/// A tuple containing the resolved findings and the number of skipped candidates.
pub fn resolve_findings(
    candidates: Vec<CandidateFinding>,
    files: &[ChangedFile],
) -> (Vec<ResolvedFinding>, usize) {
    let total = candidates.len();
    let resolved = candidates
        .into_iter()
        .filter_map(|candidate| resolve_finding(candidate, files))
        .collect::<Vec<_>>();
    let skipped = total.saturating_sub(resolved.len());
    (resolved, skipped)
}

/// Removes findings whose fingerprints have already been seen, including duplicates within the input.
///
/// # Examples
///
/// ```
/// use std::collections::BTreeSet;
///
/// let findings = deduplicate(Vec::<ResolvedFinding>::new(), &BTreeSet::<String>::new());
/// assert!(findings.is_empty());
/// ```
pub fn deduplicate(
    findings: Vec<ResolvedFinding>,
    previous: &BTreeSet<String>,
) -> Vec<ResolvedFinding> {
    let mut seen = previous.clone();
    findings
        .into_iter()
        .filter(|finding| seen.insert(finding.fingerprint.clone()))
        .collect()
}

/// Resolves a candidate finding against the changed files and determines its review location.
///
/// Unique anchor matches produce a line-level finding; missing or ambiguous anchors produce a
/// file-level finding. Returns `None` when the candidate does not correspond to a changed file.
///
/// # Examples
///
/// ```no_run
/// let resolved = resolve_finding(candidate, &files);
/// assert!(resolved.is_some());
/// ```
fn resolve_finding(candidate: CandidateFinding, files: &[ChangedFile]) -> Option<ResolvedFinding> {
    let file = files.iter().find(|file| {
        file.path == candidate.path || file.old_path.as_deref() == Some(candidate.path.as_str())
    })?;
    let mut candidate = candidate;
    candidate.path = file.path.clone();
    let fingerprint = fingerprint(&candidate);

    let anchor_lines = candidate.anchor.lines().collect::<Vec<_>>();
    if !anchor_lines.is_empty() && anchor_lines.iter().all(|line| !line.is_empty()) {
        let mut matches = Vec::new();
        for hunk in &file.hunks {
            for start in 0..hunk.lines.len() {
                if matches_anchor(&hunk.lines, start, &anchor_lines, candidate.side) {
                    let end = if let Some(end_anchor) = candidate.end_anchor.as_deref() {
                        match find_end(&hunk.lines, start, end_anchor, candidate.side) {
                            Some(value) => value,
                            None => continue,
                        }
                    } else {
                        start + anchor_lines.len() - 1
                    };
                    let Some(start_line) = side_line(&hunk.lines[start], candidate.side) else {
                        continue;
                    };
                    let Some(line) = side_line(&hunk.lines[end], candidate.side) else {
                        continue;
                    };
                    matches.push((start_line, line));
                }
            }
        }
        if matches.len() == 1 {
            let (start, line) = matches[0];
            let side = candidate.side;
            return Some(ResolvedFinding {
                candidate,
                line: Some(line),
                start_line: (start != line).then_some(start),
                side,
                fingerprint,
                file_level: false,
            });
        }
    }

    // Exact unique anchors are preferred. Ambiguous or missing anchors fall back to a
    // file-level review comment so verified findings are not silently dropped.
    Some(ResolvedFinding {
        side: candidate.side,
        candidate,
        line: None,
        start_line: None,
        fingerprint,
        file_level: true,
    })
}

/// Determines whether an anchor matches consecutive diff lines at a given position and side.
///
/// # Examples
///
/// ```
/// let lines = vec![DiffLine {
///     content: "let value = 1;".into(),
///     side: DiffSide::Right,
///     old_line: None,
///     new_line: Some(1),
/// }];
///
/// assert!(matches_anchor(
///     &lines,
///     0,
///     &["let value = 1;"],
///     DiffSide::Right,
/// ));
/// ```
fn matches_anchor(lines: &[DiffLine], start: usize, anchor: &[&str], side: DiffSide) -> bool {
    if start + anchor.len() > lines.len() {
        return false;
    }
    lines[start..start + anchor.len()]
        .iter()
        .zip(anchor)
        .all(|(line, expected)| line.content == *expected && side_matches(line.side, side))
}

/// Finds the first line at or after `start` whose content and diff side match the requested values.
///
/// # Examples
///
/// ```
/// let lines: &[DiffLine] = &[];
/// assert_eq!(find_end(lines, 0, "end", DiffSide::Right), None);
/// ```
fn find_end(lines: &[DiffLine], start: usize, end_anchor: &str, side: DiffSide) -> Option<usize> {
    lines
        .iter()
        .enumerate()
        .skip(start)
        .find(|(_, line)| line.content == end_anchor && side_matches(line.side, side))
        .map(|(index, _)| index)
}

/// Determines whether a diff line belongs to the requested side.
///
/// Context lines match either requested side.
///
/// # Examples
///
/// ```
/// assert!(side_matches(DiffSide::Right, DiffSide::Right));
/// assert!(side_matches(DiffSide::Context, DiffSide::Left));
/// assert!(!side_matches(DiffSide::Left, DiffSide::Right));
/// ```
///
/// `true` if the line side matches the requested side or is context, `false` otherwise.
fn side_matches(line_side: DiffSide, requested: DiffSide) -> bool {
    line_side == requested || line_side == DiffSide::Context
}

/// Gets the line number associated with the requested diff side.
///
/// # Examples
///
/// ```rust,ignore
/// let line_number = side_line(&line, DiffSide::Right);
/// assert_eq!(line_number, line.new_line);
/// ```
///
/// # Returns
///
/// The old-file line number for the left side, or the new-file line number for
/// the right and context sides.
fn side_line(line: &DiffLine, side: DiffSide) -> Option<u64> {
    match side {
        DiffSide::Left => line.old_line,
        DiffSide::Right | DiffSide::Context => line.new_line,
    }
}

/// Generates a stable SHA-256 fingerprint for a candidate finding.
///
/// The fingerprint incorporates the path, category, priority, normalized anchor text,
/// and normalized title.
///
/// # Examples
///
/// ```ignore
/// let fingerprint = fingerprint(&candidate);
/// assert!(!fingerprint.is_empty());
/// ```
fn fingerprint(candidate: &CandidateFinding) -> String {
    let normalized = format!(
        "{}\n{:?}\n{:?}\n{}\n{}",
        candidate.path,
        candidate.category,
        candidate.priority,
        candidate
            .anchor
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" "),
        candidate
            .title
            .to_ascii_lowercase()
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
    );
    format!("{:x}", Sha256::digest(normalized.as_bytes()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{DiffHunk, FileStatus, FindingCategory, Priority};

    #[test]
    fn resolves_exact_right_and_left_anchors() {
        let file = ChangedFile {
            path: "src/main.rs".to_owned(),
            old_path: None,
            status: FileStatus::Modified,
            patch: String::new(),
            hunks: vec![DiffHunk {
                header: "@@ -1 +1 @@".to_owned(),
                old_start: 1,
                new_start: 1,
                lines: vec![
                    DiffLine {
                        side: DiffSide::Left,
                        old_line: Some(1),
                        new_line: None,
                        content: "old".to_owned(),
                    },
                    DiffLine {
                        side: DiffSide::Right,
                        old_line: None,
                        new_line: Some(1),
                        content: "new".to_owned(),
                    },
                ],
            }],
        };
        let finding = candidate(DiffSide::Right, "new");
        let (resolved, skipped) = resolve_findings(vec![finding], &[file]);
        assert_eq!(skipped, 0);
        assert_eq!(resolved[0].line, Some(1));
        assert!(!resolved[0].file_level);
    }

    #[test]
    fn resolves_multiline_anchor_range() {
        let file = ChangedFile {
            path: "src/main.rs".to_owned(),
            old_path: None,
            status: FileStatus::Modified,
            patch: String::new(),
            hunks: vec![DiffHunk {
                header: "@@ -1 +1,2 @@".to_owned(),
                old_start: 1,
                new_start: 1,
                lines: vec![
                    DiffLine {
                        side: DiffSide::Right,
                        old_line: None,
                        new_line: Some(1),
                        content: "first".to_owned(),
                    },
                    DiffLine {
                        side: DiffSide::Right,
                        old_line: None,
                        new_line: Some(2),
                        content: "second".to_owned(),
                    },
                ],
            }],
        };
        let finding = candidate(DiffSide::Right, "first\nsecond");
        let (resolved, skipped) = resolve_findings(vec![finding], &[file]);
        assert_eq!(skipped, 0);
        assert_eq!(resolved[0].start_line, Some(1));
        assert_eq!(resolved[0].line, Some(2));
    }

    #[test]
    fn falls_back_to_file_level_for_ambiguous_anchor() {
        let repeated = DiffLine {
            side: DiffSide::Right,
            old_line: None,
            new_line: Some(1),
            content: "same".to_owned(),
        };
        let file = ChangedFile {
            path: "src/main.rs".to_owned(),
            old_path: None,
            status: FileStatus::Modified,
            patch: String::new(),
            hunks: vec![DiffHunk {
                header: "@@ -1 +1,2 @@".to_owned(),
                old_start: 1,
                new_start: 1,
                lines: vec![
                    repeated,
                    DiffLine {
                        side: DiffSide::Right,
                        old_line: None,
                        new_line: Some(2),
                        content: "same".to_owned(),
                    },
                ],
            }],
        };
        let (resolved, skipped) =
            resolve_findings(vec![candidate(DiffSide::Right, "same")], &[file]);
        assert_eq!(skipped, 0);
        assert_eq!(resolved.len(), 1);
        assert!(resolved[0].file_level);
        assert!(resolved[0].line.is_none());
    }

    #[test]
    fn resolves_renamed_path_using_old_path() {
        let file = ChangedFile {
            path: "src/new.rs".to_owned(),
            old_path: Some("src/old.rs".to_owned()),
            status: FileStatus::Renamed,
            patch: String::new(),
            hunks: vec![DiffHunk {
                header: "@@ -1 +1 @@".to_owned(),
                old_start: 1,
                new_start: 1,
                lines: vec![DiffLine {
                    side: DiffSide::Right,
                    old_line: None,
                    new_line: Some(1),
                    content: "renamed".to_owned(),
                }],
            }],
        };
        let mut finding = candidate(DiffSide::Right, "renamed");
        finding.path = "src/old.rs".to_owned();
        let (resolved, skipped) = resolve_findings(vec![finding], &[file]);
        assert_eq!(skipped, 0);
        assert_eq!(resolved[0].candidate.path, "src/new.rs");
        assert_eq!(resolved[0].line, Some(1));
    }

    fn candidate(side: DiffSide, anchor: &str) -> CandidateFinding {
        CandidateFinding {
            path: "src/main.rs".to_owned(),
            side,
            anchor: anchor.to_owned(),
            end_anchor: None,
            priority: Priority::P1,
            category: FindingCategory::Correctness,
            title: "Bug".to_owned(),
            body: "Impact".to_owned(),
            evidence: Vec::new(),
            confidence: 0.9,
        }
    }
}
