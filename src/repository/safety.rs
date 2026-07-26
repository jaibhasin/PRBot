use anyhow::{bail, Result};
use std::path::{Component, Path};

pub fn validate_ref(value: &str) -> Result<()> {
    if value.is_empty()
        || value.starts_with('-')
        || value.contains("..")
        || value.contains(char::is_whitespace)
        || value
            .chars()
            .any(|character| !character.is_ascii_alphanumeric() && !"/._-".contains(character))
    {
        bail!("unsafe Git ref '{value}'");
    }
    Ok(())
}

pub fn validate_path(value: &str) -> Result<()> {
    let path = Path::new(value);
    if path.is_absolute()
        || path
            .components()
            .any(|part| matches!(part, Component::ParentDir | Component::RootDir))
    {
        bail!("repository path escapes root: '{value}'");
    }
    Ok(())
}

pub fn truncate_chars(value: &str, max_chars: usize) -> String {
    let mut result = value.chars().take(max_chars).collect::<String>();
    if result.len() < value.len() {
        result.push_str("\n...[truncated]");
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_escaping_paths_and_unsafe_refs() {
        assert!(validate_path("../secret").is_err());
        assert!(validate_path("/etc/passwd").is_err());
        assert!(validate_ref("--upload-pack=evil").is_err());
        assert!(validate_ref("feature/safe-name").is_ok());
    }
}
