use std::collections::BTreeSet;
use tree_sitter::{Language, Node, Parser};

const MAX_SYMBOLS_PER_FILE: usize = 24;

/// Collects source symbols using language-aware parsing with lexical fallback.
///
/// # Examples
///
/// ```
/// let symbols = symbols_for("main.rs", "fn calculate(value: i32) -> i32 { value }");
/// assert!(symbols.contains(&"calculate".to_string()));
/// ```
///
/// # Returns
///
/// A sorted, de-duplicated collection of up to 24 symbols found in `source`.
///
/// `path` identifies the source language used for parsing, and `source` contains
/// the source code to analyze.
pub fn symbols_for(path: &str, source: &str) -> Vec<String> {
    language_for(path)
        .and_then(|language| syntax_symbols(language, source, true))
        .filter(|symbols| !symbols.is_empty())
        .or_else(|| language_for(path).and_then(|language| syntax_symbols(language, source, false)))
        .unwrap_or_else(|| lexical_symbols(source))
}

/// Extracts definition names from source code using the file's language when supported.
///
/// Falls back to heuristic extraction when the file type is unsupported or parsing fails.
/// The result contains at most 24 sorted, unique names.
///
/// # Examples
///
/// ```
/// let names = definitions_for("main.rs", "fn calculate(value: i32) {}");
/// assert!(names.contains(&"calculate".to_string()));
/// ```
pub fn definitions_for(path: &str, source: &str) -> Vec<String> {
    language_for(path)
        .and_then(|language| syntax_symbols(language, source, true))
        .unwrap_or_else(|| heuristic_definitions(source))
}

/// Determines whether a source line appears to define the specified symbol.
///
/// # Examples
///
/// ```
/// assert!(looks_like_definition("pub fn calculate(value: i32) {}", "calculate"));
/// assert!(!looks_like_definition("calculate(value)", "other"));
/// ```
pub fn looks_like_definition(line: &str, symbol: &str) -> bool {
    let trimmed = line.trim_start();
    if !trimmed.contains(symbol) {
        return false;
    }
    const KEYWORDS: &[&str] = &[
        "fn ",
        "func ",
        "function ",
        "def ",
        "class ",
        "struct ",
        "enum ",
        "trait ",
        "type ",
        "interface ",
        "const ",
        "let ",
        "var ",
        "pub ",
        "async ",
        "export ",
    ];
    KEYWORDS.iter().any(|keyword| trimmed.contains(keyword))
        || trimmed.starts_with(&format!("{symbol}("))
        || trimmed.starts_with(&format!("{symbol} ="))
        || trimmed.starts_with(&format!("{symbol}:"))
}

/// Extracts source symbols using the specified Tree-sitter language.
///
/// Returns up to 24 sorted symbols, or `None` if the language cannot be configured
/// or the source cannot be parsed.
///
/// # Arguments
///
/// * `language` - Tree-sitter language used to parse the source.
/// * `source` - Source code from which to extract symbols.
/// * `definitions_only` - Whether to collect only definition names.
///
/// # Examples
///
/// ```
/// let language: tree_sitter::Language = tree_sitter_rust::LANGUAGE.into();
/// let symbols = syntax_symbols(language, "fn calculate() {}", true).unwrap();
///
/// assert_eq!(symbols, vec!["calculate"]);
/// ```
fn syntax_symbols(language: Language, source: &str, definitions_only: bool) -> Option<Vec<String>> {
    let mut parser = Parser::new();
    parser.set_language(&language).ok()?;
    let tree = parser.parse(source, None)?;
    let mut symbols = BTreeSet::new();
    collect_symbols(
        tree.root_node(),
        source.as_bytes(),
        definitions_only,
        &mut symbols,
    );
    Some(symbols.into_iter().take(MAX_SYMBOLS_PER_FILE).collect())
}

/// Collects symbol names from a syntax-tree node and its descendants.
///
/// Definition names are always collected; identifier-like nodes are collected
/// only when `definitions_only` is `false`. Collection stops after the
/// per-file symbol limit is reached.
///
/// # Examples
///
/// ```
/// use std::collections::BTreeSet;
/// use tree_sitter::Parser;
///
/// let source = b"fn calculate() {}";
/// let mut parser = Parser::new();
/// parser
///     .set_language(&tree_sitter_rust::LANGUAGE.into())
///     .unwrap();
/// let tree = parser.parse(source, None).unwrap();
/// let mut symbols = BTreeSet::new();
///
/// collect_symbols(tree.root_node(), source, true, &mut symbols);
///
/// assert!(symbols.contains("calculate"));
/// ```
fn collect_symbols(
    node: Node<'_>,
    source: &[u8],
    definitions_only: bool,
    output: &mut BTreeSet<String>,
) {
    if output.len() >= MAX_SYMBOLS_PER_FILE {
        return;
    }
    let kind = node.kind();
    if is_definition_node(kind) {
        if let Some(name) = definition_name(node, source) {
            output.insert(name);
        }
    } else if !definitions_only
        && (kind == "identifier" || kind.ends_with("_identifier"))
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
        collect_symbols(child, source, definitions_only, output);
    }
}

/// Determines whether a syntax-tree node kind represents a definition or declaration.
///
/// # Examples
///
/// ```
/// assert!(is_definition_node("function_definition"));
/// assert!(is_definition_node("custom_declaration"));
/// assert!(!is_definition_node("identifier"));
/// ```
fn is_definition_node(kind: &str) -> bool {
    matches!(
        kind,
        "function_item"
            | "function_declaration"
            | "function_definition"
            | "method_declaration"
            | "method_definition"
            | "class_declaration"
            | "class_definition"
            | "struct_item"
            | "enum_item"
            | "trait_item"
            | "type_item"
            | "type_alias_declaration"
            | "interface_declaration"
            | "lexical_declaration"
            | "variable_declaration"
            | "const_item"
            | "let_declaration"
    ) || kind.contains("definition")
        || kind.contains("declaration")
}

/// Finds the first suitable identifier in a syntax-tree node or its descendants.
///
/// # Examples
///
/// ```no_run
/// let mut parser = tree_sitter::Parser::new();
/// parser.set_language(&tree_sitter_rust::LANGUAGE.into()).unwrap();
/// let tree = parser.parse("fn calculate() {}", None).unwrap();
/// let name = definition_name(tree.root_node(), b"fn calculate() {}");
/// assert_eq!(name.as_deref(), Some("calculate"));
/// ```
fn definition_name(node: Node<'_>, source: &[u8]) -> Option<String> {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        let kind = child.kind();
        if (kind == "identifier"
            || kind == "property_identifier"
            || kind == "type_identifier"
            || kind.ends_with("_identifier"))
            && child.child_count() == 0
        {
            if let Ok(value) = child.utf8_text(source) {
                if value.len() > 1 {
                    return Some(value.to_owned());
                }
            }
        }
        if let Some(nested) = definition_name(child, source) {
            return Some(nested);
        }
    }
    None
}

/// Extracts likely definition names from source lines using common declaration prefixes.
///
/// # Examples
///
/// ```
/// let source = "fn calculate() {}\nstruct Widget {}";
/// assert_eq!(
///     heuristic_definitions(source),
///     vec!["Widget".to_owned(), "calculate".to_owned()]
/// );
/// ```
fn heuristic_definitions(source: &str) -> Vec<String> {
    source
        .lines()
        .filter_map(|line| {
            let trimmed = line.trim_start();
            for prefix in [
                "fn ",
                "pub fn ",
                "async fn ",
                "def ",
                "class ",
                "struct ",
                "enum ",
                "trait ",
                "type ",
                "interface ",
                "function ",
                "const ",
                "let ",
                "var ",
                "export function ",
                "export class ",
            ] {
                if let Some(rest) = trimmed.strip_prefix(prefix) {
                    let name = rest
                        .split(|character: char| !character.is_alphanumeric() && character != '_')
                        .next()
                        .unwrap_or_default();
                    if name.len() > 1 {
                        return Some(name.to_owned());
                    }
                }
            }
            None
        })
        .collect::<BTreeSet<_>>()
        .into_iter()
        .take(MAX_SYMBOLS_PER_FILE)
        .collect()
}

/// Collects unique source tokens longer than three characters, sorted and capped per file.
///
/// # Examples
///
/// ```
/// let symbols = lexical_symbols("fn calculate_total(value) {}");
/// assert_eq!(symbols, vec!["calculate".to_owned(), "total".to_owned(), "value".to_owned()]);
/// ```
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

/// Selects a Tree-sitter language based on a file path's extension.
///
/// Supports Rust, Python, Go, JavaScript, TypeScript, and TSX files.
///
/// # Examples
///
/// ```
/// assert!(language_for("main.rs").is_some());
/// assert!(language_for("README.md").is_none());
/// ```
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_core_language_symbols_and_falls_back() {
        assert!(
            symbols_for("main.rs", "fn calculate(value: i32) { value; }")
                .contains(&"calculate".to_owned())
        );
        assert!(
            definitions_for("main.rs", "fn calculate(value: i32) { value; }")
                .contains(&"calculate".to_owned())
        );
        assert!(
            symbols_for("main.java", "class Calculator { void calculate() {} }")
                .contains(&"Calculator".to_owned())
        );
        assert!(looks_like_definition(
            "pub fn calculate(value: i32) {}",
            "calculate"
        ));
    }
}
