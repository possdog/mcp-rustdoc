//! Downloads and decompresses `.json.zst` rustdoc JSON from docs.rs.
//!
//! docs.rs only builds rustdoc JSON for crates published after 2025-05-23.
//! Older versions return 404.
//!
//! Includes a format-patching pass that handles the v56→v57 transition
//! (missing `path` field on `external_crates` entries).

use crate::error::{CratedocError, Result};
use reqwest::Client;
use rustdoc_types::Crate;
use std::io::Read;

/// Downloads rustdoc JSON from `https://docs.rs/crate/{name}/{version}/json.zst`,
/// decompresses it, and deserializes it into a `rustdoc_types::Crate`.
///
/// Retries once on transient network errors. Returns a descriptive error for
/// 404s (e.g. pre-May-2025 crate versions that lack rustdoc JSON on docs.rs).
/// Falls back to a format-patching pass when deserialization fails due to
/// known format version differences (e.g. v56→v57 `external_crates.path`).
pub struct Fetcher {
    client: Client,
}

impl Default for Fetcher {
    fn default() -> Self {
        Self::new()
    }
}

impl Fetcher {
    pub fn new() -> Self {
        Self {
            client: Client::builder()
                .user_agent("cratedoc")
                .build()
                .unwrap_or_default(),
        }
    }

    /// Fetches the rustdoc JSON from docs.rs for a given crate and version.
    /// docs.rs serves this as a zstd compressed JSON file.
    pub async fn fetch(&self, crate_name: &str, version: &str) -> Result<Crate> {
        let url = format!("https://docs.rs/crate/{}/{}/json.zst", crate_name, version);
        tracing::debug!(
            "Fetching rustdoc JSON from docs.rs for {} v{}",
            crate_name,
            version
        );

        let mut attempts = 0;
        let max_attempts = 2;
        let mut last_error = None;

        while attempts < max_attempts {
            attempts += 1;

            match self.client.get(&url).send().await {
                Ok(response) => {
                    if response.status() == reqwest::StatusCode::NOT_FOUND {
                        return Err(CratedocError::Other(format!(
                            "Documentation not found on docs.rs for {} v{}. Note that docs.rs only builds rustdoc JSON for crates published after 2025-05-23.",
                            crate_name, version
                        )));
                    }

                    let response = response.error_for_status()?;
                    let bytes = response.bytes().await?;

                    let mut decoder =
                        zstd::stream::read::Decoder::new(std::io::Cursor::new(bytes))?;
                    let mut json_str = String::new();
                    decoder.read_to_string(&mut json_str)?;

                    let krate_res: std::result::Result<Crate, _> = serde_json::from_str(&json_str);
                    let krate = match krate_res {
                        Ok(k) => k,
                        Err(e) => {
                            tracing::warn!(
                                "Failed to parse rustdoc JSON natively ({}). Attempting to patch format differences...",
                                e
                            );
                            let mut value: serde_json::Value = serde_json::from_str(&json_str)?;

                            // Patch external_crates missing `path` (format 56 to 57 transition)
                            if let Some(crates) = value
                                .get_mut("external_crates")
                                .and_then(|c| c.as_object_mut())
                            {
                                for (_, crate_val) in crates.iter_mut() {
                                    if let Some(obj) = crate_val.as_object_mut()
                                        && !obj.contains_key("path")
                                    {
                                        obj.insert(
                                            "path".to_string(),
                                            serde_json::Value::String(String::new()),
                                        );
                                    }
                                }
                            }

                            serde_json::from_value(value).map_err(|e| {
                                tracing::error!("Failed to parse even after patching: {}", e);
                                e
                            })?
                        }
                    };

                    return Ok(krate);
                }
                Err(e) => {
                    tracing::warn!(
                        "Network error fetching docs for {} v{} (attempt {}/{}): {}",
                        crate_name,
                        version,
                        attempts,
                        max_attempts,
                        e
                    );
                    last_error = Some(e);
                    if attempts < max_attempts {
                        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                    }
                }
            }
        }

        Err(CratedocError::Other(format!(
            "Failed to fetch documentation for {} v{} after {} attempts. The network might be unstable, or docs.rs may be down. Please try again later. Last error: {}",
            crate_name,
            version,
            max_attempts,
            last_error.unwrap()
        )))
    }
}
