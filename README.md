# mcp-rustdoc

> 🤖 This code was written by a human-led AI agent.

An [MCP (Model Context Protocol)](https://modelcontextprotocol.io/) server that gives AI agents rich, structured access to Rust crate documentation.

`mcp-rustdoc` bridges the gap between AI assistants (like Claude Desktop or OpenCode) and Rust codebases. Instead of having an agent hallucinate API signatures or scrape raw HTML from docs.rs, this server fetches and parses the exact `rustdoc` JSON ASTs and formats them into clean, LLM-optimized Markdown.

## Features

- **docs.rs Integration:** Automatically fetches, decompresses (`.json.zst`), and caches rustdoc JSON for published crates.
- **Local Workspace Support:** Automatically runs `cargo +nightly rustdoc` to generate documentation for your unpublished, local workspace crates on the fly (with intelligent caching to avoid slow, repeated builds).
- **Intelligent Version Resolution:** Automatically infers the correct crate version by inspecting your project's `Cargo.lock` or `Cargo.toml`.
- **Rich Formatting:** Reconstructs complex `rustdoc-types` like function signatures, struct fields, and enum variants back into readable Rust syntax.
- **Cross-References:** Automatically resolves intra-doc links (e.g., ``[`Cache`]`` becomes ``[`mcp_rustdoc::cache::Cache`]``) so agents can easily navigate the codebase.
- **Feature Gates:** Explicitly annotates items that require specific `#[cfg(feature = "...")]` flags.

## Provided Tools

The server exposes 6 powerful tools to your AI agent:

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

## Installation & Setup

### Prerequisites

You must have the **nightly** Rust toolchain installed, as generating rustdoc JSON for local crates requires unstable features.

```bash
rustup toolchain install nightly
```

### 1. Build the Server

Clone the repository and build it in release mode:

```bash
git clone https://codeberg.org/possdog/mcp-rustdoc.git
cd mcp-rustdoc
cargo build --release
```

The binary will be located at `target/release/mcp-rustdoc`.

### 2. Configure Your MCP Client

Add the built binary to your MCP client's configuration file. The server communicates over standard input/output (`stdio`).

#### For OpenCode / Claude Desktop:
Add the following to your `mcp.json` or `claude_desktop_config.json`:

```json
{
  "mcpServers": {
    "mcp-rustdoc": {
      "command": "/absolute/path/to/mcp-rustdoc/target/release/mcp-rustdoc",
      "args": []
    }
  }
}
```

### 3. Restart Your Client

Restart your MCP client (or reload the MCP connections). The agent will now automatically discover the tools and use them to answer questions about your Rust code!

## Architecture

- **`fetcher.rs`**: Downloads `.json.zst` files from the docs.rs API.
- **`cache.rs`**: Manages a local filesystem cache (using `dirs::cache_dir()`) to prevent redundant API calls to docs.rs.
- **`local.rs`**: Orchestrates `cargo metadata` and `cargo +nightly rustdoc` to generate docs for local workspace members.
- **`resolver.rs`**: Intelligently resolves omitted versions by scanning upward for `Cargo.lock` or `Cargo.toml`.
- **`render.rs`**: Contains the core logic for converting `rustdoc-types` structures into concise, Markdown-formatted text for the LLM.
- **`server.rs`**: The `rmcp` handler that defines the JSON Schemas for the tools and routes the incoming JSON-RPC calls.

## Logging

To view debug logs from the server (which are carefully routed to `stderr` to avoid corrupting the `stdio` MCP channel), you can run the server manually with `RUST_LOG=debug`:

```bash
RUST_LOG=debug cargo run --release
```

## Limitations

- **docs.rs Age Limit:** docs.rs only started building and serving rustdoc JSON for crates published *after* May 23, 2025. Older crates or older versions of crates will return a 404 error.
- **Cross-Crate Implementors:** The `implementors` tool can only find types that implement a trait *within the same crate* that is currently being queried. It cannot perform global cross-crate lookups.
