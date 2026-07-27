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
    /// Fetches a pull request's base and head revisions into a temporary bare Git repository.
    ///
    /// # Errors
    ///
    /// Returns an error if Git cannot be initialized, configured, or used to fetch the
    /// revisions, if `base_ref` is invalid, or if the fetched head does not match
    /// `expected_head`.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// let repository = GitRepository::fetch_pull_request(
    ///     "owner/project",
    ///     42,
    ///     "main",
    ///     "0123456789abcdef0123456789abcdef01234567",
    ///     "github-token",
    /// )?;
    /// # Ok::<(), anyhow::Error>(())
    /// ```
    ///
    /// `repository` must use the `owner/name` format, and `token` must be a GitHub
    /// access token.
    #[allow(clippy::too_many_arguments)]
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

    /// Creates a repository handle for an existing Git worktree.
    ///
    /// # Errors
    ///
    /// Returns an error if `path` does not contain a `.git` directory.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use std::path::Path;
    ///
    /// let repository = GitRepository::from_worktree(
    ///     Path::new("/path/to/worktree"),
    ///     "base-sha",
    ///     "head-sha",
    /// )?;
    /// # Ok::<(), anyhow::Error>(())
    /// ```
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

    /// Provides the commit SHA identified as the repository base.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use anyhow::Result;
    /// # use crate::repository::GitRepository;
    /// # fn main() -> Result<()> {
    /// let repository = GitRepository::fetch_pull_request(
    ///     "owner/repository",
    ///     42,
    ///     "main",
    ///     "expected-head-sha",
    ///     "token",
    /// )?;
    /// println!("{}", repository.base_sha());
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// Returns the base commit SHA.
    pub fn base_sha(&self) -> &str {
        &self.base_sha
    }

    /// Returns the commit SHA for the repository's head revision.
    ///
    /// # Examples
    ///
    /// ```
    /// let head_sha = repository.head_sha();
    /// assert!(!head_sha.is_empty());
    /// ```
    pub fn head_sha(&self) -> &str {
        &self.head_sha
    }

    /// Runs a Git command against the repository and returns its standard output.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// let repository = GitRepository::fetch_pull_request(
    ///     "owner/repository",
    ///     42,
    ///     "main",
    ///     "expected-head-sha",
    ///     "token",
    /// )?;
    /// let status = repository.output(["status"], "checking repository status")?;
    /// # Ok::<(), anyhow::Error>(())
    /// ```
    ///
    /// `operation` identifies the command in errors.
    ///
    /// # Errors
    ///
    /// Returns an error if the command fails or produces invalid UTF-8 output.
    pub fn output<const N: usize>(&self, args: [&str; N], operation: &str) -> Result<String> {
        output_git(&self.git_dir, args, operation)
    }

    /// Runs a Git command against the repository and returns its standard output.
    ///
    /// # Arguments
    ///
    /// * `args` - Arguments to pass to Git after the repository directory.
    /// * `operation` - Description of the operation used in error messages.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # fn main() -> anyhow::Result<()> {
    /// let repository = GitRepository::fetch_pull_request(
    ///     "owner/repository",
    ///     42,
    ///     "main",
    ///     "0123456789abcdef0123456789abcdef01234567",
    ///     "token",
    /// )?;
    /// let output = repository.output_args(&["status".to_owned()], "inspect repository")?;
    /// assert!(output.is_empty());
    /// # Ok(())
    /// # }
    /// ```
    pub fn output_args
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

    /// Reads a file from the specified repository revision, truncating its content to the requested length.
    ///
    /// `revision` must identify either the base or head revision, and `path` must be a valid repository path.
    /// The `max_bytes` limit is applied by character count.
    ///
    /// # Examples
    ///
    /// ```
    /// # // Assumes `repository` is an initialized `GitRepository`.
    /// let content = repository.read_file("head", "src/lib.rs", 4_000)?;
    /// # Ok::<(), anyhow::Error>(())
    /// ```
    ///
    /// # Errors
    ///
    /// Returns an error if the path or revision is invalid, the file cannot be read, or its content is not valid UTF-8.
    pub fn read_file(&self, revision: &str, path: &str, max_bytes: usize) -> Result<String> {
    pub fn read_file(&self, revision: &str, path: &str, max_bytes: usize) -> Result<String> {
        validate_path(path)?;
        let sha = self.revision_sha(revision)?;
        let spec = format!("{sha}:{path}");
        let content = self.output_args(&["show".to_owned(), spec], "read repository file")?;
        Ok(truncate_chars(&content, max_bytes))
    }

    /// Lists the paths in a repository tree at the specified revision.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// let repository = GitRepository::fetch_pull_request(
    ///     "owner/repository",
    ///     1,
    ///     "main",
    ///     "expected-head-sha",
    ///     "github-token",
    /// )?;
    /// let paths = repository.list_tree("head")?;
    /// # Ok::<(), anyhow::Error>(())
    /// ```
    ///
    /// # Errors
    ///
    /// Returns an error if `revision` is not `base` or `head`, or if Git cannot
    /// list the tree.
    ///
    /// # Returns
    ///
    /// The paths tracked at the specified revision.
    pub fn list_tree(&self, revision: &str) -> Result<Vec<String>> {
        let sha = self.revision_sha(revision)?;
        let output = self.output(
            ["ls-tree", "-r", "--name-only", sha],
            "list repository tree",
        )?;
        Ok(output.lines().map(str::to_owned).collect())
    }

    /// Searches a repository revision for matching text.
    ///
    /// The search is case-sensitive and treats the query as a fixed string. At most 100
    /// results are requested, and the returned output is limited to 10,000 characters.
    ///
    /// # Examples
    ///
    /// ```
    /// # use anyhow::Result;
    /// # fn example(repository: &GitRepository) -> Result<()> {
    /// let matches = repository.search("head", "needle", 20)?;
    /// # assert!(matches.len() <= 10_000);
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// # Returns
    ///
    /// The matching lines, or an empty string when no matches are found.
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

    /// Produces the diff for a path between the repository's base and head commits.
    ///
    /// # Parameters
    ///
    /// * `path` - Repository-relative path to include in the diff.
    ///
    /// # Returns
    ///
    /// The unified diff for the specified path.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # fn example() -> anyhow::Result<()> {
    /// let repository = GitRepository::fetch_pull_request(
    ///     "owner/repository",
    ///     42,
    ///     "main",
    ///     "expected-head-sha",
    ///     "github-token",
    /// )?;
    /// let diff = repository.diff_for_path("src/lib.rs")?;
    /// # Ok(())
    /// # }
    /// ```
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

    /// Lists paths changed between two Git commits.
    ///
    /// Returns an empty list when `from_sha` is empty or matches `to_sha`.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// let repository = GitRepository::fetch_pull_request(
    ///     "owner/repository",
    ///     42,
    ///     "main",
    ///     "expected-head-sha",
    ///     "token",
    /// )?;
    /// let paths = repository.changed_paths_between("base-sha", "head-sha")?;
    /// # Ok::<(), anyhow::Error>(())
    /// ```
    ///
    /// # Errors
    ///
    /// Returns an error if either SHA is not a valid Git object SHA or if Git
    /// cannot determine the changed paths.
    ///
    /// # Arguments
    ///
    /// * `from_sha` - The earlier commit SHA.
    /// * `to_sha` - The later commit SHA.
    ///
    /// # Returns
    ///
    /// A list of paths changed between the two commits.
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

    /// Searches a repository revision for symbol references or definitions.
    ///
    /// # Parameters
    ///
    /// * `revision` - The logical revision to search, such as `base` or `head`.
    /// * `query` - The symbol name or text to search for.
    /// * `max_results` - The maximum number of matches to return, capped at 100.
    /// * `definitions_only` - Whether to restrict matches to likely symbol definitions.
    ///
    /// # Returns
    ///
    /// Matching lines truncated to 10,000 characters, or an empty string when no matches are found.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # fn example() -> anyhow::Result<()> {
    /// let repository = GitRepository::fetch_pull_request(
    ///     "owner/repository",
    ///     42,
    ///     "main",
    ///     "0123456789abcdef0123456789abcdef01234567",
    ///     "token",
    /// )?;
    /// let matches = repository.search_symbol("head", "process_data", 20, true)?;
    /// # Ok(())
    /// # }
    /// ```
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

    /// Resolves a logical revision name to its corresponding commit SHA.
    ///
    /// # Errors
    ///
    /// Returns an error if `revision` is not `base` or `head`.
    ///
    /// # Examples
    ///
    /// ```rust,ignore
    /// let sha = repository.revision_sha("head")?;
    /// assert_eq!(sha, repository.head_sha());
    /// ```
    fn revision_sha(&self, revision: &str) -> Result<&str> {
        match revision {
            "base" => Ok(&self.base_sha),
            "head" => Ok(&self.head_sha),
            other => bail!("invalid revision '{other}', expected base or head"),
        }
    }
}

/// Determines whether a value has the expected format of a Git SHA.
///
/// # Examples
///
/// ```
/// assert!(looks_like_sha("0123456"));
/// assert!(!looks_like_sha("not-a-sha"));
/// ```
///
/// # Returns
///
/// `true` if the value contains 7 to 64 ASCII hexadecimal characters, `false` otherwise.
fn looks_like_sha(value: &str) -> bool {
    (7..=64).contains(&value.len()) && value.chars().all(|character| character.is_ascii_hexdigit())
}

/// Builds a regular expression pattern for matching common definitions of a symbol.
///
/// # Examples
///
/// ```
/// let pattern = definition_pattern("Widget");
/// assert!(pattern.contains("Widget"));
/// ```
///
/// # Returns
///
/// A regular expression pattern containing common definition forms for `symbol`.
///
/// # Arguments
///
/// * `symbol` - The symbol name to match.
fn definition_pattern(symbol: &str) -> String {
    let escaped = regex_escape(symbol);
    format!(
        "(fn|func|function|def|class|struct|enum|trait|type|interface|const|let|var)\\s+{escaped}\\b|{escaped}\\s*[=:({{]"
    )
}

/// Escapes regular expression metacharacters in a string.
///
/// # Examples
///
/// ```
/// assert_eq!(regex_escape("a+b"), r"a\+b");
/// ```
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

/// Finds the commit where the fetched base and head histories converge, deepening
/// the repository history when necessary.
///
/// # Arguments
///
/// * `git_dir` - Path to the bare Git repository.
/// * `auth` - Optional authentication header for fetching additional history.
/// * `base_spec` - Fetch refspec for the base revision.
/// * `head_spec` - Fetch refspec for the pull request head.
///
/// # Returns
///
/// The SHA of the merge-base commit.
///
/// # Examples
///
/// ```no_run
/// use std::path::Path;
///
/// let sha = merge_base(
///     Path::new("/path/to/repository.git"),
///     None,
///     "refs/heads/main:refs/prbot/base",
///     "refs/pull/1/head:refs/prbot/head",
/// )?;
/// # Ok::<(), anyhow::Error>(())
/// ```
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

/// Runs a Git command against a repository directory with side-effect-reducing configuration and optional GitHub authentication.
///
/// # Examples
///
/// ```no_run
/// let git_dir = std::path::Path::new("/path/to/repository.git");
/// run_git_dir(git_dir, ["status"], None, "check repository")?;
/// # Ok::<(), anyhow::Error>(())
/// ```///
///
/// `auth` supplies the GitHub HTTP authentication header when provided.
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

/// Runs a Git command for a repository and returns its UTF-8 standard output.
///
/// # Examples
///
/// ```no_run
/// use std::path::Path;
///
/// let output = output_git(
///     Path::new("/path/to/repository.git"),
///     ["rev-parse", "HEAD"],
///     "read the repository head",
/// )?;
/// # Ok::<(), anyhow::Error>(())
/// ```
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

/// Executes a command and reports an error when it fails.
///
/// # Examples
///
/// ```rust,ignore
/// let mut command = std::process::Command::new("true");
/// run_plain(&mut command, "run command")?;
/// # Ok::<(), anyhow::Error>(())
/// ```
///
/// `operation` identifies the command in any returned error.
fn run_plain(command: &mut Command, operation: &str) -> Result<()>
fn run_plain(command: &mut Command, operation: &str) -> Result<()> {
    let output = command
        .output()
        .with_context(|| format!("failed to {operation}"))?;
    parse_output(output, operation).map(|_| ())
}

/// Parses a completed command's output as UTF-8 text, reporting failures with the operation context.

///

/// # Examples

///

/// ```

/// let output = std::process::Command::new("printf")

///     .arg("ok")

///     .output()

///     .unwrap();

/// let text = parse_output(output, "read output").unwrap();

/// assert_eq!(text, "ok");

/// ```

///

/// # Errors

///

/// Returns an error when the command fails or its standard output is not valid UTF-8.
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
