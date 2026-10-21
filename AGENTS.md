# mcp-rustdoc

## Build & Test

```bash
cargo build --release          # compile
cargo clippy                   # lint
RUST_LOG=debug cargo run -- <subcommand>   # run CLI with visible logs
```

Logging goes to **stderr** — stdout carries the Markdown output (so it can
be piped into other shell commands).

## Architecture

```
src/main.rs          entrypoint: tokio runtime + tracing + CLI dispatch
src/cli.rs           clap argument parsing, Engine (component wiring + get_crate)
src/render.rs        crate JSON → LLM-friendly Markdown (~1300 lines)
src/fetcher.rs       downloads .json.zst from docs.rs
src/flight.rs        single-flight dedup for concurrent fetches (version + JSON)
src/resolver.rs      version resolution: explicit → Cargo.lock → Cargo.toml → crates.io
src/local.rs         cargo +nightly rustdoc for workspace crates
src/cache.rs         immutable filesystem cache (dirs::cache_dir())
```

## CLI

`mcp-rustdoc` is a plain CLI (clap). Subcommands map 1:1 to the former MCP
tools: `list-items`, `get-item`, `search`, `implementations`, `implementors`,
`list-workspace-crates`. Every subcommand that takes a crate name resolves its
version internally. Markdown goes to stdout; errors (item not found, path not
a module, etc.) go to stderr with exit code 1, so failures are scriptable
(`||`, `set -e`).

## Render pipeline

`src/render.rs` converts `rustdoc_types::Crate` into Markdown. Two layers:

1. **Free functions** — recursive type-level formatters:
   - `format_type` — central function; handles every `Type` variant including
     generics, lifetimes, `impl Trait`, `dyn Trait`, arrays, function pointers,
     and qualified paths.
   - `format_generic_args`, `format_generic_bounds`, `format_generics`,
     `format_where_clause`, `format_fn_signature`, `format_self_param`.

2. **`Render` struct methods** — produce complete output:
   - `list-items`, `get-item`, `search`, `implementations`, `implementors`.

### Key gotchas

- **Lifetime strings** in rustdoc JSON already include the leading `'`
  (e.g. `"'static"`, `"'a"`). Never prepend an extra `'`.
- **`format_type` is the single choke point** for type rendering. Any change
  to how a type variant renders must go through this function.
- **Source links** in `get-item` are built by `make_source_ref()` which
  handles three path formats: relative (`src/foo.rs` → docs.rs URL),
  absolute `/opt/rustwide/…` (→ dep crate docs.rs URL), and other absolute
  paths (→ plain `file:line` for local workspace use).

### `implementations` subcommand behavior

- **Struct/enum/union**: renders each inherent impl block with full method
  signatures, grouped by impl block (preserving per-block where constraints).
  Trait impls follow as a compact bulleted list.
- **Trait**: renders the trait's own interface — required methods, provided
  methods, associated types, and associated constants. This is *not* the
  same as `implementors`.

## Known limitations (do NOT spend time trying to fix)

- **Cross-crate `implementors`** — rustdoc JSON only contains impl blocks
  defined within the queried crate. Cannot list implementors defined in
  other crates. This is an architectural limit of the upstream data format,
  not a rendering bug.
- **Pre-May-2025 crates** — docs.rs only builds rustdoc JSON for crates
  published after 2025-05-23. Older versions return 404. No workaround.
- **`find_item_by_path` fallback** — when an item is not in `krate.paths`,
  falls back to matching the last `::` segment against item names. This is
  non-deterministic when items in different modules share a name. In practice
  only affects private/internal items.
- **`resolve_links`** — only handles inline `[text]` links, not `[text][id]`
  reference-style or namespace-prefixed (`[struct@Foo]`) links.

## Other conventions

- **CLI, not a server** — the binary is a one-shot CLI. Do not add daemon,
  server, or MCP transport modes without explicit request.
- **Markdown output only** — all subcommands print Markdown to stdout. Never
  HTML or raw JSON.
- **Errors on stderr, exit 1** — not-found / wrong-kind results are errors,
  never stdout placeholder text.
- **Local crates require nightly** — `cargo +nightly rustdoc` is needed for
  `--output-format json -Z unstable-options`.
- **Version resolution is baked in** — every subcommand that takes a crate
  name resolves versions internally (explicit version → Cargo.lock →
  Cargo.toml → crates.io latest). No separate call needed. crates.io lookups
  are cached in memory (24h TTL for versions, 1h for not-found misses); a
  newly published version can lag up to that TTL.
