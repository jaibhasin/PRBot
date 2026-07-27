use super::{build_context, build_manifest, GitRepository, RepositoryTools};
use crate::config::ReviewConfig;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use tempfile::TempDir;

struct Fixture {
    _temp: TempDir,
    root: PathBuf,
    base: String,
    head: String,
}

impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().expect("temp");
        let root = temp.path().to_path_buf();
        git(&root, &["init"]);
        git(&root, &["config", "user.email", "prbot@example.com"]);
        git(&root, &["config", "user.name", "PRBot"]);
        fs::create_dir_all(root.join("src")).expect("src");
        fs::create_dir_all(root.join("tests")).expect("tests");
        fs::write(
            root.join("src/lib.rs"),
            "pub fn compute(value: i32) -> i32 {\n    value + 1\n}\n",
        )
        .expect("base source");
        fs::write(
            root.join("tests/lib_test.rs"),
            "use sample::compute;\n\nfn verifies_compute() { assert_eq!(compute(1), 2); }\n",
        )
        .expect("test source");
        git(&root, &["add", "."]);
        git(&root, &["commit", "-m", "base"]);
        let base = output(&root, &["rev-parse", "HEAD"]);
        fs::write(
            root.join("src/lib.rs"),
            "pub fn compute(value: i32) -> i32 {\n    value * 2\n}\n",
        )
        .expect("head source");
        git(&root, &["commit", "-am", "change behavior"]);
        let head = output(&root, &["rev-parse", "HEAD"]);
        Self {
            _temp: temp,
            root,
            base: base.trim().to_owned(),
            head: head.trim().to_owned(),
        }
    }

    fn repository(&self) -> Arc<GitRepository> {
        Arc::new(
            GitRepository::from_worktree(&self.root, &self.base, &self.head).expect("repository"),
        )
    }
}

#[test]
fn builds_complete_manifest_and_ranks_matching_test() {
    let fixture = Fixture::new();
    let repository = fixture.repository();
    let filter = ReviewConfig::default().path_filter().expect("filter");
    let mut manifest = build_manifest(&repository, &filter).expect("manifest");
    build_context(&repository, &mut manifest).expect("context");
    assert!(manifest.complete());
    assert_eq!(manifest.eligible_hunks(), 1);
    let related = manifest
        .related_files
        .get("src/lib.rs")
        .expect("related files");
    assert!(related.iter().any(|file| file.path == "tests/lib_test.rs"));
}

#[test]
fn tools_are_revision_scoped_bounded_and_reject_traversal() {
    let fixture = Fixture::new();
    let repository = fixture.repository();
    let tools = RepositoryTools::new(Arc::clone(&repository), "PR context".to_owned());
    let base = tools
        .execute(
            "read_file",
            r#"{"path":"src/lib.rs","revision":"base","start_line":1,"end_line":5000}"#,
        )
        .expect("base file");
    assert!(base.contains("value + 1"));
    assert!(tools
        .execute(
            "read_file",
            r#"{"path":"src/lib.rs","revision":"base","start_line":500}"#,
        )
        .expect("late range")
        .is_empty());
    assert!(tools
        .execute(
            "read_file",
            r#"{"path":"../secret","revision":"head","start_line":1,"end_line":2}"#,
        )
        .is_err());
    let diff = tools
        .execute("read_diff", r#"{"paths":["src/lib.rs"]}"#)
        .expect("diff");
    assert!(diff.contains("+    value * 2"));
    assert!(tools.execute("run_shell", r#"{"command":"env"}"#).is_err());
    let symbol = tools
        .execute(
            "find_symbol",
            r#"{"query":"compute","revision":"head","max_results":20}"#,
        )
        .expect("find symbol");
    assert!(symbol.contains("compute"));
    let references = tools
        .execute(
            "find_references",
            r#"{"query":"compute","revision":"head","max_results":20}"#,
        )
        .expect("find references");
    assert!(references.contains("compute"));
    let changed = repository
        .changed_paths_between(&fixture.base, &fixture.head)
        .expect("changed paths");
    assert_eq!(changed, vec!["src/lib.rs".to_owned()]);
}

fn git(path: &Path, args: &[&str]) {
    let output = Command::new("git")
        .arg("-C")
        .arg(path)
        .args(args)
        .output()
        .expect("git");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn output(path: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(path)
        .args(args)
        .output()
        .expect("git");
    assert!(output.status.success());
    String::from_utf8(output.stdout).expect("utf8")
}
