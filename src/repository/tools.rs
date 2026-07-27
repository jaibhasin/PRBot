use crate::repository::syntax::looks_like_definition;
use crate::repository::GitRepository;
use anyhow::{bail, Context, Result};
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::Duration;

/// Executes a repository tool with a 10-second time limit.

///

/// # Examples

///

/// ```

/// # async fn example(tools: std::sync::Arc<RepositoryTools>) -> anyhow::Result<()> {

/// let result = execute_bounded(tools, "list_tree".into(), "{}".into()).await?;

/// let _output: String = result;

/// # Ok(())

/// # }

/// ```

///

/// # Errors

///

/// Returns an error if the tool exceeds the time limit, its task fails, or the

/// tool execution fails.
pub async fn execute_bounded(
    tools: Arc<RepositoryTools>,
    name: String,
    arguments: String,
) -> Result<String> {
    tokio::time::timeout(
        Duration::from_secs(10),
        tokio::task::spawn_blocking(move || tools.execute(&name, &arguments)),
    )
    .await
    .context("repository tool exceeded 10-second limit")?
    .context("repository tool task failed")?
}

pub struct RepositoryTools {
    repository: Arc<GitRepository>,
    pr_context: String,
}

impl RepositoryTools {
    /// Creates repository tools with the given repository and pull request context.
    ///
    /// # Examples
    ///
    /// ```rust,ignore
    /// let tools = RepositoryTools::new(repository, pr_context);
    /// ```
    ///
    /// # Parameters
    ///
    /// * `repository` - Repository used to perform repository operations.
    /// * `pr_context` - Contextual information associated with the pull request.
    pub fn new(repository: Arc<GitRepository>, pr_context: String) -> Self {
        Self {
            repository,
            pr_context,
        }
    }

    /// Dispatches a repository tool call by name.
    ///
    /// # Arguments
    ///
    /// * `name` - The name of the repository tool to execute.
    /// * `arguments` - The tool arguments encoded as JSON.
    ///
    /// # Returns
    ///
    /// The tool's result, or an error if the tool name or arguments are invalid.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// let tools: RepositoryTools = todo!();
    /// let result = tools.execute("get_pr_context", "{}");
    ///
    /// assert!(result.is_ok());
    /// ```
    pub fn execute(&self, name: &str, arguments: &str) -> Result<String> {
        match name {
            "list_tree" => self.list_tree(arguments),
            "read_file" => self.read_file(arguments),
            "read_diff" => self.read_diff(arguments),
            "search_code" => self.search(arguments, false, false),
            "find_symbol" => self.search(arguments, true, true),
            "find_references" => self.search(arguments, true, false),
            "get_pr_context" => Ok(truncate(&self.pr_context, 10_000)),
            other => bail!("unknown repository tool '{other}'"),
        }
    }

    /// Lists repository paths for a revision, optionally limited to a path prefix.
    ///
    /// The result contains at most 500 paths and is serialized as JSON with a maximum
    /// output length of 10,000 characters.
    ///
    /// # Examples
    ///
    /// ```
    /// let arguments = r#"{"revision":"head","prefix":"src/"}"#;
    /// assert!(arguments.contains("\"prefix\":\"src/\""));
    /// ```
    ///
    /// # Errors
    ///
    /// Returns an error if the arguments are invalid, the repository tree cannot be
    /// read, or the result cannot be serialized.
    fn list_tree(&self, arguments: &str) -> Result<String>
    fn list_tree(&self, arguments: &str) -> Result<String> {
        #[derive(Deserialize)]
        struct Args {
            #[serde(default = "head")]
            revision: String,
            #[serde(default)]
            prefix: String,
        }
        let args: Args = parse(arguments)?;
        let paths = self
            .repository
            .list_tree(&args.revision)?
            .into_iter()
            .filter(|path| path.starts_with(&args.prefix))
            .take(500)
            .collect::<Vec<_>>();
        let output = serde_json::to_string(&paths).context("failed to serialize tree result")?;
        Ok(truncate(&output, 10_000))
    }

    /// Reads a bounded range of lines from a repository file and prefixes each line with its 1-based line number.
    ///
    /// The arguments must be a JSON object containing `path`, with optional `revision`, `start_line`, and
    /// `end_line` fields. At most 400 lines and 10,000 characters are returned.
    ///
    /// # Examples
    ///
    /// ```
    /// let arguments = r#"{
    ///     "path": "src/lib.rs",
    ///     "revision": "head",
    ///     "start_line": 1,
    ///     "end_line": 20
    /// }"#;
    /// let result = tools.read_file(arguments)?;
    /// # Ok::<(), anyhow::Error>(())
    /// ```
    ///
    /// # Errors
    ///
    /// Returns an error if the arguments are invalid, the requested range is invalid, or the file cannot
    /// be read.
    fn read_file(&self, arguments: &str) -> Result<String> {
        #[derive(Deserialize)]
        struct Args {
            path: String,
            #[serde(default = "head")]
            revision: String,
            #[serde(default = "one")]
            start_line: usize,
            #[serde(default)]
            end_line: Option<usize>,
        }
        let args: Args = parse(arguments)?;
        let requested_end = args
            .end_line
            .unwrap_or_else(|| args.start_line.saturating_add(399));
        if requested_end < args.start_line {
            bail!("end_line must be greater than or equal to start_line");
        }
        let end = requested_end.min(args.start_line.saturating_add(399));
        let content = self
            .repository
            .read_file(&args.revision, &args.path, 250_000)?;
        let lines = content
            .lines()
            .enumerate()
            .filter(|(index, _)| {
                let line = index + 1;
                line >= args.start_line && line <= end
            })
            .map(|(index, line)| format!("{}: {}", index + 1, line))
            .collect::<Vec<_>>()
            .join("\n");
        Ok(truncate(&lines, 10_000))
    }

    /// Collects diffs for up to ten repository paths and limits the output to 20,000 characters.
    ///
    /// # Examples
    ///
    /// ```rust,ignore
    /// let arguments = r#"{"paths":["src/lib.rs","src/main.rs"]}"#;
    /// let diff = tools.read_diff(arguments)?;
    /// # Ok::<(), anyhow::Error>(())
    /// ```
    ///
    /// The arguments must contain a `paths` array of repository paths.
    fn read_diff(&self, arguments: &str) -> Result<String> {
        #[derive(Deserialize)]
        struct Args {
            paths: Vec<String>,
        }
        let args: Args = parse(arguments)?;
        let mut output = String::new();
        for path in args.paths.into_iter().take(10) {
            output.push_str(&self.repository.diff_for_path(&path)?);
        }
        Ok(truncate(&output, 20_000))
    }

    /// Searches repository content or symbols and optionally limits results to likely definitions.
    ///
    /// `arguments` must be a JSON object containing `query`, with optional `revision` and
    /// `max_results` fields. When `definitions_only` is enabled, results are filtered to
    /// likely definition matches and fall back to the unfiltered results if no matches remain.
    ///
    /// # Examples
    ///
    /// ```rust,ignore
    /// let results = tools.search(
    ///     r#"{"query":"RepositoryTools","revision":"head","max_results":10}"#,
    ///     true,
    ///     true,
    /// )?;
    /// # Ok::<(), anyhow::Error>(())
    /// ```
    ///
    /// # Returns
    ///
    /// The matching repository results, truncated to 10,000 characters when necessary.
    ///
    /// # Parameters
    ///
    /// * `arguments` — JSON-encoded search parameters.
    /// * `word_match` — Whether to search symbols using word matching.
    /// * `definitions_only` — Whether to retain only likely definition matches.
    fn search(&self, arguments: &str, word_match: bool, definitions_only: bool) -> Result<String> {
        #[derive(Deserialize)]
        struct Args {
            query: String,
            #[serde(default = "head")]
            revision: String,
            #[serde(default = "one_hundred")]
            max_results: usize,
        }
        let args: Args = parse(arguments)?;
        let raw = if word_match {
            self.repository.search_symbol(
                &args.revision,
                &args.query,
                args.max_results,
                definitions_only,
            )?
        } else {
            self.repository
                .search(&args.revision, &args.query, args.max_results)?
        };
        if !definitions_only {
            return Ok(raw);
        }
        let filtered = raw
            .lines()
            .filter(|line| {
                line.split_once(':')
                    .and_then(|(_, rest)| rest.split_once(':'))
                    .map(|(_, content)| looks_like_definition(content, &args.query))
                    .unwrap_or(false)
                    || looks_like_definition(line, &args.query)
            })
            .collect::<Vec<_>>()
            .join("\n");
        Ok(truncate(
            if filtered.is_empty() { &raw } else { &filtered },
            10_000,
        ))
    }
}

/// Describes the repository tools and their JSON argument schemas.
///
/// # Examples
///
/// ```
/// let definitions = tool_definitions();
/// assert_eq!(definitions.len(), 7);
/// ```
pub fn tool_definitions() -> Vec<Value>
pub fn tool_definitions() -> Vec<Value> {
    vec![
        tool(
            "list_tree",
            "List repository paths at the trusted base or PR head revision.",
            json!({"type":"object","properties":{"revision":{"type":"string","enum":["base","head"]},"prefix":{"type":"string"}}}),
        ),
        tool(
            "read_file",
            "Read at most 400 numbered lines from a repository file.",
            json!({"type":"object","required":["path"],"properties":{"path":{"type":"string"},"revision":{"type":"string","enum":["base","head"]},"start_line":{"type":"integer","minimum":1},"end_line":{"type":"integer","minimum":1}}}),
        ),
        tool(
            "read_diff",
            "Read the authoritative local diff for up to ten changed paths.",
            json!({"type":"object","required":["paths"],"properties":{"paths":{"type":"array","maxItems":10,"items":{"type":"string"}}}}),
        ),
        tool(
            "search_code",
            "Search exact text in repository source at base or head.",
            search_schema(),
        ),
        tool(
            "find_symbol",
            "Find likely definitions of a symbol using definition-oriented repository search.",
            search_schema(),
        ),
        tool(
            "find_references",
            "Find word-bounded references to a symbol across the repository.",
            search_schema(),
        ),
        tool(
            "get_pr_context",
            "Read bounded PR metadata, trusted instructions, linked issue, and check context.",
            json!({"type":"object","properties":{}}),
        ),
    ]
}

/// Builds a JSON function definition from its name, description, and parameter schema.
///
/// # Examples
///
/// ```
/// use serde_json::json;
///
/// let definition = tool("read_file", "Reads a file", json!({
///     "type": "object",
///     "properties": {}
/// }));
///
/// assert_eq!(definition["type"], "function");
/// assert_eq!(definition["function"]["name"], "read_file");
/// ```
///
/// # Arguments
///
/// * `name` - The function name.
/// * `description` - A description of the function.
/// * `parameters` - The function's JSON parameter schema.
///
/// # Returns
///
/// A JSON object containing the function definition.
fn tool(name: &str, description: &str, parameters: Value) -> Value {
    json!({
        "type": "function",
        "function": {
            "name": name,
            "description": description,
            "parameters": parameters
        }
    })
}

/// Builds the JSON schema shared by repository search tools.
///
/// # Returns
///
/// A JSON object schema requiring a search query and optionally accepting a revision and result limit.
///
/// # Examples
///
/// ```
/// let schema = search_schema();
/// assert_eq!(schema["required"][0], "query");
/// ```
fn search_schema() -> Value {
    json!({"type":"object","required":["query"],"properties":{"query":{"type":"string"},"revision":{"type":"string","enum":["base","head"]},"max_results":{"type":"integer","minimum":1,"maximum":100}}})
}

/// Parses JSON arguments into the requested type.
///
/// # Errors
///
/// Returns an error with context if `arguments` is not valid JSON or cannot be
/// deserialized into the requested type.
///
/// # Examples
///
/// ```
/// #[derive(serde::Deserialize)]
/// struct Args {
///     query: String,
/// }
///
/// let args: Args = parse(r#"{"query":"rust"}"#).unwrap();
/// assert_eq!(args.query, "rust");
/// ```
fn parse<T: for<'de> Deserialize<'de>>(arguments: &str) -> Result<T> {
    serde_json::from_str(arguments).context("invalid repository tool arguments")
}

/// Limits a string to a maximum number of Unicode scalar values and marks truncated output.
///
/// # Examples
///
/// ```
/// assert_eq!(truncate("hello", 3), "hel\n...[truncated]");
/// assert_eq!(truncate("hello", 10), "hello");
/// ```
fn truncate(value: &str, max_chars: usize) -> String
fn truncate(value: &str, max_chars: usize) -> String {
    let mut output = value.chars().take(max_chars).collect::<String>();
    if output.chars().count() < value.chars().count() {
        output.push_str("\n...[truncated]");
    }
    output
}

/// Provides the default repository revision name.
///
/// # Examples
///
/// ```
/// assert_eq!(head(), "head");
/// ```
fn head() -> String {
    "head".to_owned()
}

/// Provides the default starting line number.
///
/// # Examples
///
/// ```
/// assert_eq!(one(), 1);
/// ```
fn one() -> usize {
    1
}

/// Provides the default maximum number of search results.
///
/// # Examples
///
/// ```
/// assert_eq!(one_hundred(), 100);
/// ```
fn one_hundred() -> usize {
    100
}
