//! Generates rustdoc JSON for local (unpublished) workspace crates via
//! `cargo +nightly rustdoc --output-format json -Z unstable-options`.
//!
//! Includes staleness detection based on `Cargo.toml` and `src/` file
//! modification times. Requires the **nightly** toolchain.

use crate::error::{McpRustdocError, Result};
use rustdoc_types::Crate;
use serde::Deserialize;
use std::env;
use std::path::PathBuf;
use tokio::process::Command;

#[derive(Deserialize)]
struct Metadata {
    packages: Vec<Package>,
    target_directory: String,
    workspace_members: Vec<String>,
}

#[derive(Deserialize)]
struct Package {
    id: String,
    name: String,
    version: String,
    manifest_path: String,
}

/// Generates rustdoc JSON for local workspace crates via
/// `cargo +nightly rustdoc --output-format json -Z unstable-options`.
///
/// Staleness detection compares the mtime of the generated JSON against
/// `Cargo.toml` and all files under `src/`. Requires the **nightly**
/// toolchain.
pub struct Local;

impl Default for Local {
    fn default() -> Self {
        Self::new()
    }
}

impl Local {
    pub fn new() -> Self {
        Self
    }

    /// Tries to generate rustdoc JSON for a local workspace crate.
    /// Returns Ok(Some((Crate, version))) if successful.
    /// Returns Ok(None) if the crate is not part of the local workspace.
    pub async fn generate(&self, crate_name: &str) -> Result<Option<(Crate, String)>> {
        let cwd = env::current_dir()?;

        // 1. Run cargo metadata to find workspace members and target directory
        let output = Command::new("cargo")
            .arg("metadata")
            .arg("--format-version=1")
            .arg("--no-deps")
            .current_dir(&cwd)
            .output()
            .await?;

        if !output.status.success() {
            return Ok(None); // Not in a cargo workspace or error running metadata
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let metadata: Metadata = serde_json::from_str(&stdout).unwrap_or_else(|_| Metadata {
            packages: vec![],
            target_directory: String::new(),
            workspace_members: vec![],
        });

        // 2. Find the crate in the workspace packages
        let pkg = metadata
            .packages
            .iter()
            .find(|p| p.name == crate_name && metadata.workspace_members.contains(&p.id));

        let pkg = match pkg {
            Some(p) => p,
            None => return Ok(None),
        };

        // 4. Find the generated JSON file
        // Rustdoc replaces dashes with underscores in the filename
        let rustdoc_name = crate_name.replace("-", "_");
        let json_path = PathBuf::from(&metadata.target_directory)
            .join("doc")
            .join(format!("{}.json", rustdoc_name));

        // Check if we can skip generation
        let should_generate = if json_path.exists() {
            let json_mtime = std::fs::metadata(&json_path)
                .and_then(|m| m.modified())
                .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
            let manifest_mtime = std::fs::metadata(&pkg.manifest_path)
                .and_then(|m| m.modified())
                .unwrap_or(std::time::SystemTime::UNIX_EPOCH);

            // Re-generate if Cargo.toml is newer than the JSON
            if manifest_mtime > json_mtime {
                true
            } else {
                // Also check if src/ is newer
                let src_dir = PathBuf::from(&pkg.manifest_path)
                    .parent()
                    .unwrap()
                    .join("src");
                if src_dir.exists() {
                    let mut is_newer = false;
                    for entry in walkdir::WalkDir::new(&src_dir)
                        .into_iter()
                        .filter_map(|e| e.ok())
                    {
                        if let Ok(metadata) = entry.metadata()
                            && let Ok(mtime) = metadata.modified()
                            && mtime > json_mtime
                        {
                            is_newer = true;
                            break;
                        }
                    }
                    is_newer
                } else {
                    false
                }
            }
        } else {
            true
        };

        if should_generate {
            // 3. Run cargo +nightly rustdoc
            // cargo +nightly rustdoc --output-format json -Z unstable-options --manifest-path {path}/Cargo.toml
            let rustdoc_output = Command::new("cargo")
                .arg("+nightly")
                .arg("rustdoc")
                .arg("--output-format")
                .arg("json")
                .arg("-Z")
                .arg("unstable-options")
                .arg("--manifest-path")
                .arg(&pkg.manifest_path)
                .output()
                .await?;

            if !rustdoc_output.status.success() {
                let stderr = String::from_utf8_lossy(&rustdoc_output.stderr);
                if stderr.contains("toolchain 'nightly'") && stderr.contains("is not installed") {
                    return Err(McpRustdocError::Other(
                        "Nightly toolchain is required to generate rustdoc JSON. Please run: rustup toolchain install nightly".to_string()
                    ));
                }
                return Err(McpRustdocError::Other(format!(
                    "Failed to generate local rustdoc for {}: {}",
                    crate_name, stderr
                )));
            }
        }

        if !json_path.exists() {
            return Err(McpRustdocError::Other(format!(
                "Expected rustdoc JSON at {:?} but it was not found.",
                json_path
            )));
        }

        // 5. Parse and return
        let content = std::fs::read_to_string(&json_path)?;
        let krate: Crate = serde_json::from_str(&content)?;

        Ok(Some((krate, pkg.version.clone())))
    }

    /// Return a Markdown listing of all workspace member crates with their
    /// versions and manifest paths. Uses `cargo metadata` under the hood.
    pub async fn list_workspace_crates(&self) -> Result<String> {
        let cwd = env::current_dir()?;

        let output = Command::new("cargo")
            .arg("metadata")
            .arg("--format-version=1")
            .arg("--no-deps")
            .current_dir(&cwd)
            .output()
            .await?;

        if !output.status.success() {
            return Err(McpRustdocError::Other(
                "Failed to run cargo metadata".to_string(),
            ));
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let metadata: Metadata = serde_json::from_str(&stdout).unwrap_or_else(|_| Metadata {
            packages: vec![],
            target_directory: String::new(),
            workspace_members: vec![],
        });

        let mut output_str = String::from("# Workspace Crates\n\n");
        if metadata.workspace_members.is_empty() {
            output_str.push_str("No crates found in workspace.\n");
            return Ok(output_str);
        }

        for p in metadata.packages {
            if metadata.workspace_members.contains(&p.id) {
                output_str.push_str(&format!(
                    "- **{}** (v{})\n  - Path: {}\n",
                    p.name, p.version, p.manifest_path
                ));
            }
        }

        Ok(output_str)
    }
}
