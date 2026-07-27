mod primary;
mod verifier;

pub use primary::{review_prompt, reviewer_system};
pub use verifier::{verification_prompt, verifier_system};

fn finding_schema() -> &'static str {
    r#"{"findings":[{"path":"src/file.rs","side":"RIGHT|LEFT","anchor":"exact changed line without diff prefix","end_anchor":null,"priority":"P0|P1|P2|P3","category":"correctness|architecture|security|reliability|compatibility|performance|concurrency|api|documentation|other","title":"concise title","body":"why this fails, triggering conditions, impact, and a focused fix","evidence":[{"path":"src/related.rs","revision":"base|head","start_line":1,"end_line":2,"explanation":"supporting evidence"}],"confidence":0.0}]}"#
}
