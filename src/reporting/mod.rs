mod anchors;
mod summary;

pub use anchors::{deduplicate, resolve_findings};
pub use summary::{
    finding_body, parse_summary_state, render_review_body, render_summary, SUMMARY_MARKER,
};
