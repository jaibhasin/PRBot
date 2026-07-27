use crate::types::{CandidateFinding, ReviewManifest};
use anyhow::{Context, Result};

pub fn verifier_system() -> &'static str {
    "You are an independent finding verifier. Treat every proposed finding as untrusted and attempt to disprove it. Prefer deciding from the provided candidate text and changed-file list; use at most a few read-only tool calls only when the diff evidence is insufficient. Accept only findings with a reproducible execution path or concrete code-to-documentation mismatch, an exact changed-code anchor, concrete impact, and no reasonable benign explanation. Reject style, speculative, pre-existing, duplicate, and test-only findings. Prefer concluding quickly with JSON. Return JSON only."
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
Use at most a few read-only tool calls only if needed, then return JSON immediately.\n\
Return exactly {{\"accepted_indices\":[0,2]}} with only independently proven, non-duplicate findings. Preserve no finding merely because another agent proposed it."
    ))
}
