use anyhow::{bail, Context, Result};
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use tempfile::TempDir;

use super::safety::{truncate_chars, validate_path, validate_ref};

pub struct GitRepository {
    _temp: Option<TempDir>,
    git_dir: PathBuf,
    base_sha: String,
    head_sha: String,
}

impl GitRepository {
    pub fn fetch_pull_request(
        repository: &str,
        pr_number: u64,
        base_ref: &str,
        expected_head: &str,
        token: &str,
    ) -> Result<Self> {
        validate_ref(base_ref)?;
        let temp = tempfile::tempdir().context("failed to create temporary Git repository")?;
        let git_dir = temp.path().join("repository.git");
        run_plain(
            Command::new("git").args(["init", "--bare"]).arg(&git_dir),
            "initialize Git",
        )?;
        let url = format!("https://github.com/{repository}.git");
        run_git_dir(
            &git_dir,
            ["remote", "add", "origin", url.as_str()],
            None,
            "configure Git remote",
        )?;

        let auth = format!(
            "AUTHORIZATION: basic {}",
            STANDARD.encode(format!("x-access-token:{token}"))
        );
        let base_spec = format!("+refs/heads/{base_ref}:refs/prbot/base");
        let head_spec = format!("+refs/pull/{pr_number}/head:refs/prbot/head");
        run_git_dir(
            &git_dir,
            [
                "fetch",
                "--no-tags",
                "--depth=100",
                "origin",
                base_spec.as_str(),
                head_spec.as_str(),
            ],
            Some(&auth),
            "fetch pull request revisions",
        )?;

        let head_sha = output_git(
            &git_dir,
            ["rev-parse", "refs/prbot/head"],
            "resolve PR head",
        )?;
        if head_sha.trim() != expected_head {
            bail!(
                "fetched PR head {} does not match GitHub head {expected_head}",
                head_sha.trim()
            );
        }
        let base_sha = merge_base(&git_dir, Some(&auth), &base_spec, &head_spec)?;
        Ok(Self {
            _temp: Some(temp),
            git_dir,
            base_sha,
            head_sha: head_sha.trim().to_owned(),
        })
    }

    #[cfg(test)]
    pub fn from_worktree(path: &Path, base_sha: &str, head_sha: &str) -> Result<Self> {
        let git_dir = path.join(".git");
        if !git_dir.exists() {
            bail!("test repository has no .git directory");
        }
        Ok(Self {
            _temp: None,
            git_dir,
            base_sha: base_sha.to_owned(),
            head_sha: head_sha.to_owned(),
        })
    }

    pub fn base_sha(&self) -> &str {
        &self.base_sha
    }

    pub fn head_sha(&self) -> &str {
        &self.head_sha
    }

    pub fn output<const N: usize>(&self, args: [&str; N], operation: &str) -> Result<String> {
        output_git(&self.git_dir, args, operation)
    }

    pub fn output_args(&self, args: &[String], operation: &str) -> Result<String> {
        let output = Command::new("git")
            .arg("--git-dir")
            .arg(&self.git_dir)
            .args(args)
            .env("GIT_OPTIONAL_LOCKS", "0")
            .output()
            .with_context(|| format!("failed to {operation}"))?;
        parse_output(output, operation)
    }

    pub fn read_file(&self, revision: &str, path: &str, max_bytes: usize) -> Result<String> {
        validate_path(path)?;
        let sha = self.revision_sha(revision)?;
        let spec = format!("{sha}:{path}");
        let content = self.output_args(&["show".to_owned(), spec], "read repository file")?;
        Ok(truncate_chars(&content, max_bytes))
    }

    pub fn list_tree(&self, revision: &str) -> Result<Vec<String>> {
        let sha = self.revision_sha(revision)?;
        let output = self.output(
            ["ls-tree", "-r", "--name-only", sha],
            "list repository tree",
        )?;
        Ok(output.lines().map(str::to_owned).collect())
    }

    pub fn search(&self, revision: &str, query: &str, max_results: usize) -> Result<String> {
        if query.trim().is_empty() {
            bail!("search query cannot be empty");
        }
        let sha = self.revision_sha(revision)?;
        let args = vec![
            "grep".to_owned(),
            "-n".to_owned(),
            "-I".to_owned(),
            "-F".to_owned(),
            "-m".to_owned(),
            max_results.min(100).to_string(),
            "-e".to_owned(),
            query.to_owned(),
            sha.to_owned(),
            "--".to_owned(),
        ];
        match self.output_args(&args, "search repository") {
            Ok(output) => Ok(truncate_chars(&output, 10_000)),
            Err(error) if error.to_string().contains("exit status 1") => Ok(String::new()),
            Err(error) => Err(error),
        }
    }

    pub fn diff_for_path(&self, path: &str) -> Result<String> {
        validate_path(path)?;
        self.output_args(
            &[
                "diff".to_owned(),
                "--no-ext-diff".to_owned(),
                "--find-renames".to_owned(),
                "--unified=40".to_owned(),
                self.base_sha.clone(),
                self.head_sha.clone(),
                "--".to_owned(),
                path.to_owned(),
            ],
            "read file diff",
        )
    }

    pub fn changed_paths_between(&self, from_sha: &str, to_sha: &str) -> Result<Vec<String>> {
        if from_sha.trim().is_empty() || from_sha == to_sha {
            return Ok(Vec::new());
        }
        if !looks_like_sha(from_sha) || !looks_like_sha(to_sha) {
            bail!("changed_paths_between requires full Git object SHAs");
        }
        let output = self.output_args(
            &[
                "diff".to_owned(),
                "--name-only".to_owned(),
                "-z".to_owned(),
                "-M".to_owned(),
                from_sha.to_owned(),
                to_sha.to_owned(),
            ],
            "list paths changed since previous review",
        )?;
        Ok(output
            .split('\0')
            .filter(|path| !path.is_empty())
            .map(str::to_owned)
            .collect())
    }

    pub fn search_symbol(
        &self,
        revision: &str,
        query: &str,
        max_results: usize,
        definitions_only: bool,
    ) -> Result<String> {
        if query.trim().is_empty() {
            bail!("search query cannot be empty");
        }
        let sha = self.revision_sha(revision)?;
        let mut args = vec!["grep".to_owned(), "-n".to_owned(), "-I".to_owned()];
        if definitions_only {
            args.push("-E".to_owned());
            args.push("-e".to_owned());
            args.push(definition_pattern(query));
        } else {
            args.push("-w".to_owned());
            args.push("-F".to_owned());
            args.push("-e".to_owned());
            args.push(query.to_owned());
        }
        args.push("-m".to_owned());
        args.push(max_results.min(100).to_string());
        args.push(sha.to_owned());
        args.push("--".to_owned());
        match self.output_args(&args, "search repository symbols") {
            Ok(output) => Ok(truncate_chars(&output, 10_000)),
            Err(error) if error.to_string().contains("exit status 1") => Ok(String::new()),
            Err(error) => Err(error),
        }
    }

    fn revision_sha(&self, revision: &str) -> Result<&str> {
        match revision {
            "base" => Ok(&self.base_sha),
            "head" => Ok(&self.head_sha),
            other => bail!("invalid revision '{other}', expected base or head"),
        }
    }
}

fn looks_like_sha(value: &str) -> bool {
    (7..=64).contains(&value.len()) && value.chars().all(|character| character.is_ascii_hexdigit())
}

fn definition_pattern(symbol: &str) -> String {
    let escaped = regex_escape(symbol);
    format!(
        "(fn|func|function|def|class|struct|enum|trait|type|interface|const|let|var)\\s+{escaped}\\b|{escaped}\\s*[=:({{]"
    )
}

fn regex_escape(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if "\\.^$|?*+()[]{}".contains(character) {
                format!("\\{character}")
            } else {
                character.to_string()
            }
        })
        .collect()
}

fn merge_base(
    git_dir: &Path,
    auth: Option<&str>,
    base_spec: &str,
    head_spec: &str,
) -> Result<String> {
    if let Ok(value) = output_git(
        git_dir,
        ["merge-base", "refs/prbot/base", "refs/prbot/head"],
        "find merge base",
    ) {
        return Ok(value.trim().to_owned());
    }
    for deepen in [200, 500, 1_000] {
        let deepen_arg = format!("--deepen={deepen}");
        run_git_dir(
            git_dir,
            [
                "fetch",
                "--no-tags",
                deepen_arg.as_str(),
                "origin",
                base_spec,
                head_spec,
            ],
            auth,
            "deepen pull request history",
        )?;
        if let Ok(value) = output_git(
            git_dir,
            ["merge-base", "refs/prbot/base", "refs/prbot/head"],
            "find merge base",
        ) {
            return Ok(value.trim().to_owned());
        }
    }
    run_git_dir(
        git_dir,
        [
            "fetch",
            "--no-tags",
            "--unshallow",
            "origin",
            base_spec,
            head_spec,
        ],
        auth,
        "deepen pull request history",
    )?;
    Ok(output_git(
        git_dir,
        ["merge-base", "refs/prbot/base", "refs/prbot/head"],
        "find merge base",
    )?
    .trim()
    .to_owned())
}

fn run_git_dir<const N: usize>(
    git_dir: &Path,
    args: [&str; N],
    auth: Option<&str>,
    operation: &str,
) -> Result<()> {
    let mut command = Command::new("git");
    command.arg("--git-dir").arg(git_dir).args(args);
    command.env("GIT_OPTIONAL_LOCKS", "0");
    command.env("GIT_LFS_SKIP_SMUDGE", "1");
    command.env("GIT_CONFIG_COUNT", if auth.is_some() { "3" } else { "2" });
    command.env("GIT_CONFIG_KEY_0", "core.hooksPath");
    command.env("GIT_CONFIG_VALUE_0", "/dev/null");
    command.env("GIT_CONFIG_KEY_1", "protocol.file.allow");
    command.env("GIT_CONFIG_VALUE_1", "never");
    if let Some(value) = auth {
        command.env("GIT_CONFIG_KEY_2", "http.https://github.com/.extraheader");
        command.env("GIT_CONFIG_VALUE_2", value);
    }
    run_plain(&mut command, operation)
}

fn output_git<const N: usize>(git_dir: &Path, args: [&str; N], operation: &str) -> Result<String> {
    let output = Command::new("git")
        .arg("--git-dir")
        .arg(git_dir)
        .args(args)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .output()
        .with_context(|| format!("failed to {operation}"))?;
    parse_output(output, operation)
}

fn run_plain(command: &mut Command, operation: &str) -> Result<()> {
    let output = command
        .output()
        .with_context(|| format!("failed to {operation}"))?;
    parse_output(output, operation).map(|_| ())
}

fn parse_output(output: Output, operation: &str) -> Result<String> {
    if !output.status.success() {
        bail!(
            "failed to {operation} ({}): {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    String::from_utf8(output.stdout).with_context(|| format!("{operation} returned non-UTF-8 data"))
}
