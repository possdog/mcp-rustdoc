//! Immutable filesystem cache for rustdoc JSON.
//!
//! Uses `dirs::cache_dir()` for a platform-appropriate cache directory
//! (e.g. `~/Library/Caches/mcp-rustdoc/` on macOS). Entries are keyed
//! by `(crate_name, version, target)` and never expire — a given
//! published crate version is immutable.

use crate::error::{McpRustdocError, Result};
use rustdoc_types::Crate;
use std::fs;
use std::path::PathBuf;

/// Filesystem-backed cache for deserialized `rustdoc_types::Crate` data.
///
/// Cache layout: `{system_cache_dir}/mcp-rustdoc/{crate_name}/{version}/{target}.json`
/// On macOS this resolves to `~/Library/Caches/mcp-rustdoc/`.
///
/// Entries are immutable — once written they live on disk forever, which is
/// sound for published crate docs (a given crate+version never changes).
pub struct Cache {
    base_dir: PathBuf,
}

impl Cache {
    pub fn new() -> Result<Self> {
        let cache_dir = dirs::cache_dir().ok_or_else(|| {
            McpRustdocError::CacheError("Could not determine system cache directory".to_string())
        })?;
        let base_dir = cache_dir.join("mcp-rustdoc");
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
