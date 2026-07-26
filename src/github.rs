//! GitHub API helpers (diff fetch, comments, reviews).
//!
//! Implemented in a later step. This module exists so the crate layout
//! already matches the Action architecture.

#![allow(dead_code)]

/// Marker type for the future GitHub client.
pub struct GitHubClient {
    pub token: String,
}

impl GitHubClient {
    pub fn new(token: impl Into<String>) -> Self {
        Self {
            token: token.into(),
        }
    }
}
