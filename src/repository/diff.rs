use crate::config::PathFilter;
use crate::repository::GitRepository;
use crate::types::{
    ChangedFile, DiffHunk, DiffLine, DiffSide, FileStatus, IgnoredFile, ReviewManifest,
};
use anyhow::{Context, Result};

pub fn build_manifest(repository: &GitRepository, filter: &PathFilter) -> Result<ReviewManifest> {
    let names = repository.output_args(
        &[
            "diff".to_owned(),
            "--name-status".to_owned(),
            "-M".to_owned(),
            repository.base_sha().to_owned(),
            repository.head_sha().to_owned(),
        ],
        "list changed files",
    )?;
    let mut manifest = ReviewManifest::default();
    for line in names.lines().filter(|line| !line.trim().is_empty()) {
        let (status, old_path, path) = parse_name_status(line)?;
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

fn parse_name_status(line: &str) -> Result<(FileStatus, Option<String>, String)> {
    let fields = line.split('\t').collect::<Vec<_>>();
    let raw_status = fields.first().copied().unwrap_or_default();
    let code = raw_status.chars().next().unwrap_or('?');
    let status = match code {
        'A' => FileStatus::Added,
        'M' => FileStatus::Modified,
        'D' => FileStatus::Deleted,
        'R' => FileStatus::Renamed,
        'C' => FileStatus::Copied,
        _ => FileStatus::Unknown,
    };
    if matches!(status, FileStatus::Renamed | FileStatus::Copied) {
        let old = fields.get(1).context("rename missing old path")?;
        let new = fields.get(2).context("rename missing new path")?;
        Ok((status, Some((*old).to_owned()), (*new).to_owned()))
    } else {
        let path = fields.get(1).context("changed file missing path")?;
        Ok((status, None, (*path).to_owned()))
    }
}

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

fn parse_hunk_header(header: &str) -> Option<(u64, u64)> {
    let mut parts = header.split_whitespace();
    parts.next()?;
    let old = parts.next()?.strip_prefix('-')?;
    let new = parts.next()?.strip_prefix('+')?;
    Some((parse_start(old)?, parse_start(new)?))
}

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
}
