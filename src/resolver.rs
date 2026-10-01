//! Resolves crate versions from the local project context or crates.io.
//!
//! The resolution waterfall is: explicit version → Cargo.lock → Cargo.toml
//! → crates.io API (latest published version). All paths search upward from
//! `cwd`, so behavior depends on where the MCP server process was started.

use crate::error::{McpRustdocError, Result};
use reqwest::Client;
use std::env;
use std::path::{Path, PathBuf};

/// Resolves a crate version from several sources, in priority order:
/// 1. An explicitly provided version (returned as-is).
/// 2. `Cargo.lock` — the exact version pinned in the current project.
/// 3. `Cargo.toml` — the specified version from `[dependencies]` with
///    leading sigils (`^`, `~`, etc.) stripped.
/// 4. The crates.io API — the latest version published to crates.io.
pub struct Resolver {
    client: Client,
}

// Subset of the `GET https://crates.io/api/v1/crates/{name}` response.
#[derive(serde::Deserialize)]
struct CratesIoResponse {
    #[serde(rename = "crate")]
    krate: CratesIoCrate,
}

#[derive(serde::Deserialize)]
struct CratesIoCrate {
    max_version: String,
}

impl Default for Resolver {
    fn default() -> Self {
        Self::new()
    }
}

impl Resolver {
    pub fn new() -> Self {
        Self {
            client: Client::builder()
                .user_agent("mcp-rustdoc")
                .build()
                .unwrap_or_default(),
        }
    }

    /// Resolve a version string for `crate_name`. See the struct-level docs
    /// for the resolution waterfall.
    pub async fn resolve_version(
        &self,
        crate_name: &str,
        provided_version: Option<&str>,
    ) -> Result<String> {
        if let Some(v) = provided_version {
            return Ok(v.to_string());
        }

        let cwd = env::current_dir()?;

        // 1. Cargo.lock
        if let Some(lock_path) = self.find_file_upwards(&cwd, "Cargo.lock")
            && let Ok(version) = self.extract_from_lockfile(&lock_path, crate_name)
        {
            return Ok(version);
        }

        // 2. Cargo.toml
        if let Some(toml_path) = self.find_file_upwards(&cwd, "Cargo.toml")
            && let Ok(version) = self.extract_from_cargo_toml(&toml_path, crate_name)
        {
            // Docs.rs can sometimes handle semantic versions in the URL (e.g., 1.0)
            // but it's safer to fetch the exact latest if we can't parse an exact version.
            // For now, if we found it in Cargo.toml, we can just return it, though
            // it might have '^' or '~' which might fail in docs.rs URL.
            // Let's strip '^', '~', '=', '>', '<' to make a basic guess,
            // or just rely on the crates.io fallback.
            let clean_version = version.trim_start_matches(|c| "^~=>< ".contains(c));
            return Ok(clean_version.to_string());
        }

        // 3. Latest from crates.io
        self.fetch_latest_from_crates_io(crate_name).await
    }

    /// Walk up the directory tree from `start_dir` looking for `file_name`.
    fn find_file_upwards(&self, start_dir: &Path, file_name: &str) -> Option<PathBuf> {
        let mut current_dir = start_dir;
        loop {
            let file_path = current_dir.join(file_name);
            if file_path.exists() {
                return Some(file_path);
            }
            match current_dir.parent() {
                Some(parent) => current_dir = parent,
                None => break,
            }
        }
        None
    }

    /// Search a `Cargo.lock` file for `crate_name` and return its pinned version.
    fn extract_from_lockfile(&self, path: &Path, crate_name: &str) -> Result<String> {
        let lockfile = cargo_lock::Lockfile::load(path)
            .map_err(|e| McpRustdocError::Other(format!("Failed to parse Cargo.lock: {}", e)))?;

        for package in lockfile.packages {
            if package.name.as_str() == crate_name {
                return Ok(package.version.to_string());
            }
        }

        Err(McpRustdocError::Other(format!(
            "Crate {} not found in Cargo.lock",
            crate_name
        )))
    }

    /// Search a `Cargo.toml` file for `crate_name` in `[dependencies]`,
    /// `[dev-dependencies]`, and `[build-dependencies]`. Returns the raw
    /// version string (which may be a semver range like `^1.0` or `>=1.2,
    /// <2.0` — the caller is responsible for stripping sigils).
    fn extract_from_cargo_toml(&self, path: &Path, crate_name: &str) -> Result<String> {
        let content = std::fs::read_to_string(path)?;
        let toml: toml::Value = toml::from_str(&content)?;

        // Try dependencies, dev-dependencies, build-dependencies
        let tables = ["dependencies", "dev-dependencies", "build-dependencies"];

        for table in tables {
            if let Some(deps) = toml.get(table).and_then(|t| t.as_table())
                && let Some(dep) = deps.get(crate_name)
            {
                if let Some(version) = dep.as_str() {
                    return Ok(version.to_string());
                } else if let Some(dep_table) = dep.as_table()
                    && let Some(version) = dep_table.get("version").and_then(|v| v.as_str())
                {
                    return Ok(version.to_string());
                }
            }
        }

        Err(McpRustdocError::Other(format!(
            "Crate {} not found in Cargo.toml",
            crate_name
        )))
    }

    /// Query `https://crates.io/api/v1/crates/{name}` for the latest published
    /// version. This is used as the final fallback when the crate is not in the
    /// local project context. (The previous `https://docs.rs/crate/{name}/json`
    /// endpoint stopped returning JSON metadata and now serves HTML.)
    async fn fetch_latest_from_crates_io(&self, crate_name: &str) -> Result<String> {
        let url = format!("https://crates.io/api/v1/crates/{}", crate_name);

        let response = self.client.get(&url).send().await?;

        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Err(McpRustdocError::Other(format!(
                "Crate {} not found on crates.io. It might be a local workspace crate or misspelled.",
                crate_name
            )));
        }

        let response: CratesIoResponse = response.error_for_status()?.json().await?;
        Ok(response.krate.max_version)
    }
}
