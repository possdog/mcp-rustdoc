//! Unified error type for all mcp-rustdoc operations.
//!
//! `McpRustdocError` wraps cache, I/O, network, JSON, and TOML errors
//! plus a catch-all `Other` variant for ad-hoc error messages.

use thiserror::Error;

#[derive(Error, Debug)]
pub enum McpRustdocError {
    #[error("Cache error: {0}")]
    CacheError(String),
    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),
    #[error("Network error: {0}")]
    NetworkError(#[from] reqwest::Error),
    #[error("JSON error: {0}")]
    JsonError(#[from] serde_json::Error),
    #[error("TOML error: {0}")]
    TomlError(#[from] toml::de::Error),
    #[error("Other error: {0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, McpRustdocError>;
