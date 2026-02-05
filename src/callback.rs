//! Placeholder module - publish logic moved to scripts/publish-to-gh-pages.sh
//! This keeps streamstress focused on build/deploy/test.

/// No-op function for backward compatibility.
pub fn maybe_publish_results() {
    // Publishing is handled by the container entrypoint script,
    // not by streamstress itself.
}
