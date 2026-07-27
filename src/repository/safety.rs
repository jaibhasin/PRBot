use anyhow::{bail, Result};
use std::path::{Component, Path};

/// Validates a Git reference for safe use.
///
/// A valid reference is nonempty, does not start with `-`, does not contain
/// `..` or whitespace, and contains only ASCII letters, digits, `/`, `.`, `_`,
/// or `-`.
///
/// # Examples
///
/// ```
/// assert!(validate_ref("feature/safe-name").is_ok());
/// assert!(validate_ref("../unsafe").is_err());
/// ```
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

/// Validates that a repository path stays within the intended root.
///
/// # Errors
///
/// Returns an error if the path is absolute or contains a parent-directory
/// component.
///
/// # Examples
///
/// ```
/// assert!(validate_path("src/lib.rs").is_ok());
/// assert!(validate_path("../secret").is_err());
/// ```
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

/// Truncates a string to at most the specified number of characters.
///
/// Appends a truncation marker when the resulting string is shorter than the
/// original string by byte length.
///
/// # Examples
///
/// ```
/// assert_eq!(
///     truncate_chars("hello world", 5),
///     "hello\n...[truncated]"
/// );
/// ```
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
