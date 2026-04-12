use crate::error::{McpRustdocError, Result};
use reqwest::Client;
use std::env;
use std::path::{Path, PathBuf};

pub struct Resolver {
    client: Client,
}

#[derive(serde::Deserialize)]
struct DocsRsMetadata {
    version: String,
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
        if let Some(lock_path) = self.find_file_upwards(&cwd, "Cargo.lock") {
            if let Ok(version) = self.extract_from_lockfile(&lock_path, crate_name) {
                return Ok(version);
            }
        }

        // 2. Cargo.toml
        if let Some(toml_path) = self.find_file_upwards(&cwd, "Cargo.toml") {
            if let Ok(version) = self.extract_from_cargo_toml(&toml_path, crate_name) {
                // Docs.rs can sometimes handle semantic versions in the URL (e.g., 1.0)
                // but it's safer to fetch the exact latest if we can't parse an exact version.
                // For now, if we found it in Cargo.toml, we can just return it, though
                // it might have '^' or '~' which might fail in docs.rs URL.
                // Let's strip '^', '~', '=', '>', '<' to make a basic guess,
                // or just rely on the docs.rs fallback.
                let clean_version = version.trim_start_matches(|c| "^~=>< ".contains(c));
                return Ok(clean_version.to_string());
            }
        }

        // 3. Latest from docs.rs
        self.fetch_latest_from_docs_rs(crate_name).await
    }

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

    fn extract_from_cargo_toml(&self, path: &Path, crate_name: &str) -> Result<String> {
        let content = std::fs::read_to_string(path)?;
        let toml: toml::Value = toml::from_str(&content)?;

        // Try dependencies, dev-dependencies, build-dependencies
        let tables = ["dependencies", "dev-dependencies", "build-dependencies"];

        for table in tables {
            if let Some(deps) = toml.get(table).and_then(|t| t.as_table()) {
                if let Some(dep) = deps.get(crate_name) {
                    if let Some(version) = dep.as_str() {
                        return Ok(version.to_string());
                    } else if let Some(dep_table) = dep.as_table() {
                        if let Some(version) = dep_table.get("version").and_then(|v| v.as_str()) {
                            return Ok(version.to_string());
                        }
                    }
                }
            }
        }

        Err(McpRustdocError::Other(format!(
            "Crate {} not found in Cargo.toml",
            crate_name
        )))
    }

    async fn fetch_latest_from_docs_rs(&self, crate_name: &str) -> Result<String> {
        let url = format!("https://docs.rs/crate/{}/json", crate_name);

        let response = self.client.get(&url).send().await?;

        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Err(McpRustdocError::Other(format!(
                "Crate {} not found on docs.rs. It might be a local workspace crate or misspelled.",
                crate_name
            )));
        }

        let metadata: DocsRsMetadata = response.error_for_status()?.json().await?;
        Ok(metadata.version)
    }
}
