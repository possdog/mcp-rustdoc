//! CLI argument parsing and command dispatch.
//!
//! Each subcommand renders one view of a crate's documentation
//! (`list-items`, `get-item`, `search`, `implementations`, `implementors`,
//! `list-workspace-crates`). All subcommands that take a crate resolve its
//! version internally (explicit `--version` → local workspace → Cargo.lock
//! → Cargo.toml → crates.io latest) — see `Engine::get_crate`.

use crate::cache::Cache;
use crate::error::Result;
use crate::fetcher::Fetcher;
use crate::local::Local;
use crate::render::Render;
use crate::resolver::Resolver;

use clap::{Parser, Subcommand};
use rustdoc_types::Crate;

#[derive(Parser)]
#[command(
    name = "cratedoc",
    version,
    about = "Browse Rust crate documentation from docs.rs and local workspaces as Markdown",
    subcommand_negates_reqs = true
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand)]
pub enum Command {
    /// List items in a crate or module — structs, enums, functions, traits,
    /// modules, constants, type aliases, etc.
    ListItems {
        /// Crate name (e.g. `serde`), or a local workspace crate name
        crate_name: String,
        /// Optional module path to list items within (defaults to crate root)
        item_path: Option<String>,
        /// Crate version (defaults to local workspace → Cargo.lock → Cargo.toml → crates.io latest)
        #[arg(long)]
        version: Option<String>,
    },
    /// Get detailed documentation for a specific item — includes full doc
    /// comments, type signature, struct fields, enum variants, function
    /// parameters, feature gates, and code examples.
    GetItem {
        /// Crate name
        crate_name: String,
        /// Fully-qualified item path (e.g. `serde::Deserialize`)
        item_path: String,
        /// Crate version (defaults to local workspace → Cargo.lock → Cargo.toml → crates.io latest)
        #[arg(long)]
        version: Option<String>,
    },
    /// Full-text search across item names and doc comments with fuzzy matching.
    Search {
        /// Crate name
        crate_name: String,
        /// Search query
        query: String,
        /// Maximum number of results
        #[arg(long, default_value_t = 10)]
        limit: usize,
        /// Crate version (defaults to local workspace → Cargo.lock → Cargo.toml → crates.io latest)
        #[arg(long)]
        version: Option<String>,
    },
    /// For a given type, return all of its impl blocks — both inherent
    /// methods and trait implementations.
    Implementations {
        /// Crate name
        crate_name: String,
        /// Fully-qualified type path (e.g. `serde_json::Value`)
        item_path: String,
        /// Crate version (defaults to local workspace → Cargo.lock → Cargo.toml → crates.io latest)
        #[arg(long)]
        version: Option<String>,
    },
    /// For a given trait, return all concrete types that implement it.
    Implementors {
        /// Crate name
        crate_name: String,
        /// Fully-qualified trait path (e.g. `serde::Serialize`)
        trait_path: String,
        /// Crate version (defaults to local workspace → Cargo.lock → Cargo.toml → crates.io latest)
        #[arg(long)]
        version: Option<String>,
    },
    /// List all crates in the current workspace with their versions and
    /// local paths.
    ListWorkspaceCrates,
}

/// The core engine — holds the component instances (cache, fetcher, local,
/// resolver, render) and implements the crate-acquisition waterfall shared
/// by every documentation subcommand.
pub struct Engine {
    cache: std::sync::Arc<Cache>,
    fetcher: std::sync::Arc<Fetcher>,
    local: std::sync::Arc<Local>,
    resolver: std::sync::Arc<Resolver>,
    render: std::sync::Arc<Render>,
}

impl Engine {
    pub fn new() -> Result<Self> {
        Ok(Self {
            cache: std::sync::Arc::new(Cache::new()?),
            fetcher: std::sync::Arc::new(Fetcher::new()),
            local: std::sync::Arc::new(Local::new()),
            resolver: std::sync::Arc::new(Resolver::new()),
            render: std::sync::Arc::new(Render::new()),
        })
    }

    /// Obtain a parsed `rustdoc_types::Crate` for the requested crate and version.
    ///
    /// Resolution order (waterfall):
    /// 1. If no version provided, try local workspace crate generation first.
    /// 2. Resolve version via `Resolver` (Cargo.lock → Cargo.toml → crates.io latest).
    /// 3. Check disk cache.
    /// 4. Fetch from docs.rs and cache the result.
    async fn get_crate(&self, crate_name: &str, version: Option<&str>) -> Result<Crate> {
        if version.is_none()
            && let Ok(Some((krate, _))) = self.local.generate(crate_name).await
        {
            return Ok(krate);
        }

        let resolved_version = self.resolver.resolve_version(crate_name, version).await?;
        let target = "x86_64-unknown-linux-gnu";

        if let Ok(Some(krate)) = self.cache.get(crate_name, &resolved_version, target) {
            return Ok(krate);
        }

        let krate = self.fetcher.fetch(crate_name, &resolved_version).await?;
        let _ = self
            .cache
            .set(crate_name, &resolved_version, target, &krate);

        Ok(krate)
    }

    /// Execute a subcommand and return its Markdown output.
    pub async fn run(&self, command: Command) -> Result<String> {
        match command {
            Command::ListItems {
                crate_name,
                item_path,
                version,
            } => {
                let krate = self.get_crate(&crate_name, version.as_deref()).await?;
                self.render.list_items(&krate, item_path.as_deref())
            }
            Command::GetItem {
                crate_name,
                item_path,
                version,
            } => {
                let krate = self.get_crate(&crate_name, version.as_deref()).await?;
                self.render.get_item(&krate, &item_path)
            }
            Command::Search {
                crate_name,
                query,
                limit,
                version,
            } => {
                let krate = self.get_crate(&crate_name, version.as_deref()).await?;
                Ok(self.render.search(&krate, &query, limit))
            }
            Command::Implementations {
                crate_name,
                item_path,
                version,
            } => {
                let krate = self.get_crate(&crate_name, version.as_deref()).await?;
                self.render.implementations(&krate, &item_path)
            }
            Command::Implementors {
                crate_name,
                trait_path,
                version,
            } => {
                let krate = self.get_crate(&crate_name, version.as_deref()).await?;
                self.render.implementors(&krate, &trait_path)
            }
            Command::ListWorkspaceCrates => self.local.list_workspace_crates().await,
        }
    }
}
