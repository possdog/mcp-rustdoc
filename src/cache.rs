//! Immutable filesystem cache for rustdoc JSON.
//!
//! Uses `dirs::cache_dir()` for a platform-appropriate cache directory
//! (e.g. `~/Library/Caches/cratedoc/` on macOS). Entries are keyed
//! by `(crate_name, version, target)` and never expire — a given
//! published crate version is immutable.

use crate::error::{CratedocError, Result};
use rustdoc_types::Crate;
use std::fs;
use std::path::PathBuf;

/// Filesystem-backed cache for deserialized `rustdoc_types::Crate` data.
///
/// Cache layout: `{system_cache_dir}/cratedoc/{crate_name}/{version}/{target}.json`
/// On macOS this resolves to `~/Library/Caches/cratedoc/`.
///
/// Entries are immutable — once written they live on disk forever, which is
/// sound for published crate docs (a given crate+version never changes).
///
/// On first use, a legacy `mcp-rustdoc` cache directory (from when this tool
/// was an MCP server) is renamed to the new `cratedoc` directory so existing
/// cached crates are not orphaned behind a re-download.
pub struct Cache {
    base_dir: PathBuf,
}

impl Cache {
    pub fn new() -> Result<Self> {
        let cache_dir = dirs::cache_dir().ok_or_else(|| {
            CratedocError::CacheError("Could not determine system cache directory".to_string())
        })?;
        let base_dir = cache_dir.join("cratedoc");

        // One-time migration from the pre-rename directory. Rename only if
        // the new directory does not exist yet; otherwise we'd have to merge
        // trees, and the legacy entries would be stale anyway.
        if !base_dir.exists() {
            let legacy_dir = cache_dir.join("mcp-rustdoc");
            if legacy_dir.exists() {
                // Fallible on cross-device or missing-permission edge cases —
                // the cache is best-effort, so a failure just means a re-fetch.
                let _ = fs::rename(&legacy_dir, &base_dir);
            }
        }

        Ok(Self { base_dir })
    }

    fn get_cache_path(&self, crate_name: &str, version: &str, target: &str) -> PathBuf {
        self.base_dir
            .join(crate_name)
            .join(version)
            .join(format!("{}.json", target))
    }

    pub fn get(&self, crate_name: &str, version: &str, target: &str) -> Result<Option<Crate>> {
        let path = self.get_cache_path(crate_name, version, target);
        if !path.exists() {
            return Ok(None);
        }

        let content = fs::read_to_string(&path)?;
        let krate: Crate = serde_json::from_str(&content)?;
        Ok(Some(krate))
    }

    pub fn set(&self, crate_name: &str, version: &str, target: &str, krate: &Crate) -> Result<()> {
        let path = self.get_cache_path(crate_name, version, target);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }

        let content = serde_json::to_string(krate)?;
        fs::write(&path, content)?;
        Ok(())
    }
}
