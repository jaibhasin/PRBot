use std::collections::BTreeSet;
use tree_sitter::{Language, Node, Parser};

const MAX_SYMBOLS_PER_FILE: usize = 24;

pub fn symbols_for(path: &str, source: &str) -> Vec<String> {
    language_for(path)
        .and_then(|language| syntax_symbols(language, source, true))
        .filter(|symbols| !symbols.is_empty())
        .or_else(|| language_for(path).and_then(|language| syntax_symbols(language, source, false)))
        .unwrap_or_else(|| lexical_symbols(source))
}

pub fn definitions_for(path: &str, source: &str) -> Vec<String> {
    language_for(path)
        .and_then(|language| syntax_symbols(language, source, true))
        .unwrap_or_else(|| heuristic_definitions(source))
}

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
