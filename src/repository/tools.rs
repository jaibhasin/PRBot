use crate::repository::GitRepository;
use anyhow::{bail, Context, Result};
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::Duration;

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
    pub fn new(repository: Arc<GitRepository>, pr_context: String) -> Self {
        Self {
            repository,
            pr_context,
        }
    }

    pub fn execute(&self, name: &str, arguments: &str) -> Result<String> {
        match name {
            "list_tree" => self.list_tree(arguments),
            "read_file" => self.read_file(arguments),
            "read_diff" => self.read_diff(arguments),
            "search_code" | "find_symbol" | "find_references" => self.search(arguments),
            "get_pr_context" => Ok(truncate(&self.pr_context, 10_000)),
            other => bail!("unknown repository tool '{other}'"),
        }
    }

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

    fn search(&self, arguments: &str) -> Result<String> {
        #[derive(Deserialize)]
        struct Args {
            query: String,
            #[serde(default = "head")]
            revision: String,
            #[serde(default = "one_hundred")]
            max_results: usize,
        }
        let args: Args = parse(arguments)?;
        self.repository
            .search(&args.revision, &args.query, args.max_results)
    }
}

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
            "Find definitions or uses of a symbol using bounded repository search.",
            search_schema(),
        ),
        tool(
            "find_references",
            "Find references to a changed symbol using bounded repository search.",
            search_schema(),
        ),
        tool(
            "get_pr_context",
            "Read bounded PR metadata, trusted instructions, linked issue, and check context.",
            json!({"type":"object","properties":{}}),
        ),
    ]
}

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

fn search_schema() -> Value {
    json!({"type":"object","required":["query"],"properties":{"query":{"type":"string"},"revision":{"type":"string","enum":["base","head"]},"max_results":{"type":"integer","minimum":1,"maximum":100}}})
}

fn parse<T: for<'de> Deserialize<'de>>(arguments: &str) -> Result<T> {
    serde_json::from_str(arguments).context("invalid repository tool arguments")
}

fn truncate(value: &str, max_chars: usize) -> String {
    let mut output = value.chars().take(max_chars).collect::<String>();
    if output.chars().count() < value.chars().count() {
        output.push_str("\n...[truncated]");
    }
    output
}

fn head() -> String {
    "head".to_owned()
}

fn one() -> usize {
    1
}

fn one_hundred() -> usize {
    100
}
