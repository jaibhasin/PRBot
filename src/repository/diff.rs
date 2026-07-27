use crate::config::PathFilter;
use crate::repository::GitRepository;
use crate::types::{
    ChangedFile, DiffHunk, DiffLine, DiffSide, FileStatus, IgnoredFile, ReviewManifest,
};
use anyhow::{Context, Result};

/// Builds a review manifest from the changes between the repository's base and head revisions.
///
/// Files that are not reviewable or have no textual diff hunks are recorded as ignored.
///
/// # Errors
///
/// Returns an error if Git operations fail or the changed-file metadata cannot be parsed.
///
/// # Examples
///
/// ```no_run
/// let manifest = build_manifest(&repository, &filter)?;
/// assert!(manifest.files.iter().all(|file| !file.hunks.is_empty()));
/// # Ok::<(), anyhow::Error>(())
/// ```
///
/// `repository` supplies the revision range and file patches; `filter` determines which paths are reviewable.
pub fn build_manifest(repository: &GitRepository, filter: &PathFilter) -> Result<ReviewManifest> {
    let names = repository.output_args(
        &[
            "diff".to_owned(),
            "--name-status".to_owned(),
            "-z".to_owned(),
            "-M".to_owned(),
            repository.base_sha().to_owned(),
            repository.head_sha().to_owned(),
        ],
        "list changed files",
    )?;
    let mut manifest = ReviewManifest::default();
    for (status, old_path, path) in parse_name_status_output(&names)? {
        if !filter.is_reviewable(&path) {
            manifest.ignored.push(IgnoredFile {
                path,
                reason: "excluded, generated, binary, or unsupported file".to_owned(),
            });
            continue;
        }
        let patch = repository.diff_for_path(&path)?;
        let hunks = parse_patch(&patch);
        if hunks.is_empty() {
            manifest.ignored.push(IgnoredFile {
                path,
                reason: "no textual diff hunks".to_owned(),
            });
            continue;
        }
        manifest.files.push(ChangedFile {
            path,
            old_path,
            status,
            patch,
            hunks,
        });
    }
    Ok(manifest)
}

/// Parses NUL-delimited Git name-status output into file change entries.
///
/// Rename and copy entries include both their original and current paths.
///
/// # Errors
///
/// Returns an error when a changed file lacks a path or a rename/copy entry
/// lacks either its original or current path.
///
/// # Examples
///
/// ```
/// # use anyhow::Result;
/// # fn example() -> Result<()> {
/// let entries = parse_name_status_output("M\\0src/lib.rs\\0")?;
/// assert_eq!(entries.len(), 1);
/// # Ok(())
/// # }
/// ```
fn parse_name_status_output(output: &str) -> Result<Vec<(FileStatus, Option<String>, String)>> {
    let mut fields = output.split('\0').filter(|field| !field.is_empty());
    let mut result = Vec::new();
    while let Some(raw_status) = fields.next() {
        let status = status_from_code(raw_status);
        if matches!(status, FileStatus::Renamed | FileStatus::Copied) {
            let old = fields.next().context("rename missing old path")?;
            let new = fields.next().context("rename missing new path")?;
            result.push((status, Some(old.to_owned()), new.to_owned()));
        } else {
            let path = fields.next().context("changed file missing path")?;
            result.push((status, None, path.to_owned()));
        }
    }
    Ok(result)
}

/// Converts a Git name-status code into its corresponding file status.
///
/// Unknown or empty codes are mapped to [`FileStatus::Unknown`].
///
/// # Examples
///
/// ```
/// assert!(matches!(status_from_code("M"), FileStatus::Modified));
/// assert!(matches!(status_from_code(""), FileStatus::Unknown));
/// ```
fn status_from_code(raw_status: &str) -> FileStatus {
    match raw_status.chars().next().unwrap_or('?') {
        'A' => FileStatus::Added,
        'M' => FileStatus::Modified,
        'D' => FileStatus::Deleted,
        'R' => FileStatus::Renamed,
        'C' => FileStatus::Copied,
        _ => FileStatus::Unknown,
    }
}

/// Parses unified diff text into hunks and their individual line changes.
///
/// Lines before the first hunk header and no-newline markers are ignored. Invalid
/// hunk headers use zero-based starting positions.
///
/// # Examples
///
/// ```
/// let hunks = parse_patch("@@ -1 +1 @@\n-old\n+new");
///
/// assert_eq!(hunks.len(), 1);
/// assert_eq!(hunks[0].lines.len(), 2);
/// assert_eq!(hunks[0].lines[0].content, "old");
/// assert_eq!(hunks[0].lines[1].content, "new");
/// ```
pub fn parse_patch(patch: &str) -> Vec<DiffHunk> {
    let mut hunks = Vec::new();
    let mut current: Option<DiffHunk> = None;
    let mut old_line = 0_u64;
    let mut new_line = 0_u64;

    for line in patch.lines() {
        if line.starts_with("@@ ") {
            if let Some(hunk) = current.take() {
                hunks.push(hunk);
            }
            let (old_start, new_start) = parse_hunk_header(line).unwrap_or((0, 0));
            old_line = old_start;
            new_line = new_start;
            current = Some(DiffHunk {
                header: line.to_owned(),
                old_start,
                new_start,
                lines: Vec::new(),
            });
            continue;
        }
        let Some(hunk) = current.as_mut() else {
            continue;
        };
        if line.starts_with("\\ No newline") {
            continue;
        }
        let (side, old, new, content) = if let Some(value) = line.strip_prefix('+') {
            let new = new_line;
            new_line += 1;
            (DiffSide::Right, None, Some(new), value)
        } else if let Some(value) = line.strip_prefix('-') {
            let old = old_line;
            old_line += 1;
            (DiffSide::Left, Some(old), None, value)
        } else {
            let value = line.strip_prefix(' ').unwrap_or(line);
            let old = old_line;
            let new = new_line;
            old_line += 1;
            new_line += 1;
            (DiffSide::Context, Some(old), Some(new), value)
        };
        hunk.lines.push(DiffLine {
            side,
            old_line: old,
            new_line: new,
            content: content.to_owned(),
        });
    }
    if let Some(hunk) = current {
        hunks.push(hunk);
    }
    hunks
}

/// Extracts the starting old and new line numbers from a unified-diff hunk header.
///
/// Returns `None` when the header does not contain valid old and new ranges.
///
/// # Examples
///
/// ```
/// assert_eq!(parse_hunk_header("@@ -10,2 +12,3 @@"), Some((10, 12)));
/// assert_eq!(parse_hunk_header("invalid"), None);
/// ```
fn parse_hunk_header(header: &str) -> Option<(u64, u64)> {
    let mut parts = header.split_whitespace();
    parts.next()?;
    let old = parts.next()?.strip_prefix('-')?;
    let new = parts.next()?.strip_prefix('+')?;
    Some((parse_start(old)?, parse_start(new)?))
}

/// Extracts the starting line number from a diff range.
///
/// # Examples
///
/// ```
/// assert_eq!(parse_start("12,5"), Some(12));
/// assert_eq!(parse_start("7"), Some(7));
/// ```
///
/// # Returns
///
/// The parsed starting line number, or `None` if the range is invalid.
///
/// # Examples
///
/// ```
/// assert_eq!(parse_start("invalid"), None);
/// ```
fn parse_start(value: &str) -> Option<u64> {
    value.split(',').next()?.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_added_deleted_and_context_lines() {
        let patch = "@@ -10,2 +10,3 @@ fn main()\n same\n-old\n+new\n+extra";
        let hunks = parse_patch(patch);
        assert_eq!(hunks.len(), 1);
        assert_eq!(hunks[0].lines[1].old_line, Some(11));
        assert_eq!(hunks[0].lines[2].new_line, Some(11));
        assert_eq!(hunks[0].lines[3].new_line, Some(12));
    }

    #[test]
    fn parses_rename_name_status() {
        let mut entries =
            parse_name_status_output("R100\0src/old.rs\0src/new.rs\0").expect("rename");
        let (status, old, new) = entries.remove(0);
        assert!(matches!(status, FileStatus::Renamed));
        assert_eq!(old.as_deref(), Some("src/old.rs"));
        assert_eq!(new, "src/new.rs");
    }

    #[test]
    fn parses_nul_delimited_path_with_tabs() {
        let entries = parse_name_status_output("M\0src/a\tb.rs\0").expect("NUL name status");
        assert_eq!(entries[0].2, "src/a\tb.rs");
    }

    #[test]
    fn ignores_no_newline_marker() {
        let patch =
            "@@ -1 +1 @@\n-old\n\\ No newline at end of file\n+new\n\\ No newline at end of file";
        let hunks = parse_patch(patch);
        assert_eq!(hunks[0].lines.len(), 2);
        assert_eq!(hunks[0].lines[1].new_line, Some(1));
    }
}
