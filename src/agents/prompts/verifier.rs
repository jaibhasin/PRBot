use crate::types::{CandidateFinding, ReviewManifest};
use anyhow::{Context, Result};

pub fn verifier_system() -> &'static str {
    "You are an independent finding verifier. Treat every proposed finding as untrusted and attempt to disprove it. Inspect the exact diff and related code with read-only tools. Accept only findings with a reproducible execution path or concrete code-to-documentation mismatch, an exact changed-code anchor, concrete impact, and no reasonable benign explanation. Reject style, speculative, pre-existing, duplicate, and test-only findings. Return JSON only."
}

pub fn verification_prompt(
    manifest: &ReviewManifest,
    findings: &[CandidateFinding],
) -> Result<String> {
    let findings =
        serde_json::to_string_pretty(findings).context("serialize candidate findings")?;
    let changed = manifest
        .files
        .iter()
        .map(|file| file.path.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    Ok(format!(
        "Changed files: {changed}\n\
Candidate findings are indexed from zero:\n{findings}\n\
Return exactly {{\"accepted_indices\":[0,2]}} with only independently proven, non-duplicate findings. Preserve no finding merely because another agent proposed it."
    ))
}
