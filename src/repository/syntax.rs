use std::collections::BTreeSet;
use tree_sitter::{Language, Node, Parser};

const MAX_SYMBOLS_PER_FILE: usize = 24;

pub fn symbols_for(path: &str, source: &str) -> Vec<String> {
    language_for(path)
        .and_then(|language| syntax_symbols(language, source))
        .unwrap_or_else(|| lexical_symbols(source))
}

fn syntax_symbols(language: Language, source: &str) -> Option<Vec<String>> {
    let mut parser = Parser::new();
    parser.set_language(&language).ok()?;
    let tree = parser.parse(source, None)?;
    let mut symbols = BTreeSet::new();
    collect_identifiers(tree.root_node(), source.as_bytes(), &mut symbols);
    Some(symbols.into_iter().take(MAX_SYMBOLS_PER_FILE).collect())
}

fn collect_identifiers(node: Node<'_>, source: &[u8], output: &mut BTreeSet<String>) {
    if output.len() >= MAX_SYMBOLS_PER_FILE {
        return;
    }
    let kind = node.kind();
    if (kind == "identifier" || kind.ends_with("_identifier"))
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
        collect_identifiers(child, source, output);
    }
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
            symbols_for("main.java", "class Calculator { void calculate() {} }")
                .contains(&"Calculator".to_owned())
        );
    }
}
