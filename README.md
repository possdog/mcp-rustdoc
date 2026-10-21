# mcp-rustdoc

> 🤖 This code was written by a human-led AI agent.

A CLI that gives humans, AI agents, and shell scripts rich, structured access to Rust crate documentation.

Instead of hallucinating API signatures or scraping raw HTML from docs.rs, `mcp-rustdoc` fetches and parses the exact `rustdoc` JSON ASTs and formats them into clean, LLM-optimized Markdown. It works as a plain command-line tool: agents learn it on demand via a `SKILL.md` instead of carrying tool definitions in every session, and its stdout-only output can be piped straight into `grep`, `head`, or other commands.

## Features

- **docs.rs Integration:** Automatically fetches, decompresses (`.json.zst`), and caches rustdoc JSON for published crates.
- **Local Workspace Support:** Automatically runs `cargo +nightly rustdoc` to generate documentation for your unpublished, local workspace crates on the fly (with intelligent caching to avoid slow, repeated builds).
- **Intelligent Version Resolution:** Automatically infers the correct crate version by inspecting your project's `Cargo.lock` or `Cargo.toml`.
- **Rich Formatting:** Reconstructs complex `rustdoc-types` like function signatures, struct fields, and enum variants back into readable Rust syntax.
- **Cross-References:** Automatically resolves intra-doc links (e.g., ``[`Cache`]`` becomes ``[`mcp_rustdoc::cache::Cache`]``) so agents can easily navigate the codebase.
- **Feature Gates:** Explicitly annotates items that require specific `#[cfg(feature = "...")]` flags.

## Commands

The binary exposes 6 subcommands:

1. **`list-workspace-crates`**
   - Lists all crates in the current workspace with their versions, local paths, and availability status.
2. **`list-items`**
   - Lists all items (structs, enums, functions, traits, modules, etc.) inside a specific crate or module.
3. **`get-item`**
   - Gets detailed documentation for a specific item, including full doc comments, type signatures, struct fields, enum variants, and feature gates.
4. **`search`**
   - Performs a fast, multi-word fuzzy search across all item names and doc comments within a crate.
5. **`implementations`**
   - Returns all `impl` blocks (both inherent methods and trait implementations) for a given struct, enum, or union.
6. **`implementors`**
   - Returns all concrete types that implement a given trait.

All crate-taking subcommands accept `--version <VERSION>` to pin a release; without it, the version is resolved from local workspace context, `Cargo.lock`, `Cargo.toml`, or crates.io latest.

## Piping

Markdown goes to stdout, and logs/errors go to stderr, so output composes with other shell commands:

```bash
mcp-rustdoc list-items serde | grep module
mcp-rustdoc search tokio runtime --limit 20 | head -30
mcp-rustdoc get-item serde_json serde_json::Value | less
mcp-rustdoc get-item serde serde::Serialize || echo "not found"
```

A missing item or crate is an error: message on stderr, exit code 1.

## Installation & Setup

### Prerequisites

You must have the **nightly** Rust toolchain installed, as generating rustdoc JSON for local crates requires unstable features.

```bash
rustup toolchain install nightly
```

### Build

Clone the repository and build it in release mode:

```bash
git clone https://codeberg.org/possdog/mcp-rustdoc.git
cd mcp-rustdoc
cargo build --release
```

The binary will be located at `target/release/mcp-rustdoc` — copy or symlink it onto your `PATH`, or install it directly with `cargo install --path .`.

### Using with an AI agent

A matching `SKILL.md` ships in the repository root. Agents that support skills (e.g. Mistral Vibe) load it on demand, so the tool costs no context until it is actually needed. Copy or symlink the skill into your skills directory to make it available everywhere:

```bash
mkdir -p ~/.vibe/skills/rustdoc
ln -s "$(pwd)/SKILL.md" ~/.vibe/skills/rustdoc/SKILL.md
```

The skill body documents every subcommand, so the agent can discover exact usage at run time.

## Architecture

- **`fetcher.rs`**: Downloads `.json.zst` files from the docs.rs API.
- **`cache.rs`**: Manages a local filesystem cache (using `dirs::cache_dir()`) to prevent redundant API calls to docs.rs.
- **`local.rs`**: Orchestrates `cargo metadata` and `cargo +nightly rustdoc` to generate docs for local workspace members.
- **`resolver.rs`**: Intelligently resolves omitted versions by scanning upward for `Cargo.lock` or `Cargo.toml`.
- **`render.rs`**: Contains the core logic for converting `rustdoc-types` structures into concise, Markdown-formatted text for the LLM.
- **`cli.rs`**: Defines the subcommands (clap) and routes each one to the rendering engine.
- **`main.rs`**: Entrypoint — Tokio runtime, tracing, and dispatch.

## Logging

Logs are carefully routed to `stderr` (stdout carries only Markdown output, so it stays pipeable). To view debug logs, run with `RUST_LOG=debug`:

```bash
RUST_LOG=debug cargo run --release -- search serde error
```

## Limitations

- **docs.rs Age Limit:** docs.rs only started building and serving rustdoc JSON for crates published *after* May 23, 2025. Older crates or older versions of crates will return a 404 error.
- **Cross-Crate Implementors:** The `implementors` tool can only find types that implement a trait *within the same crate* that is currently being queried. It cannot perform global cross-crate lookups.
