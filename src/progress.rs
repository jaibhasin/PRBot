use std::env;
use std::sync::atomic::{AtomicBool, Ordering};

static STEP_LOG: AtomicBool = AtomicBool::new(false);

/// Enables or disables PRBot step logs for this process.
pub fn configure(enabled: bool) {
    STEP_LOG.store(enabled, Ordering::Relaxed);
}

/// Returns whether step logs should be written to stderr.
pub fn enabled() -> bool {
    if STEP_LOG.load(Ordering::Relaxed) {
        return true;
    }
    matches!(
        env::var("PRBOT_STEP_LOG").ok().as_deref(),
        Some("1" | "true" | "TRUE" | "yes" | "YES")
    )
}

/// Prints one progress line to stderr when step logging is enabled.
pub fn step(message: impl AsRef<str>) {
    if enabled() {
        eprintln!("PRBot: {}", message.as_ref());
    }
}
