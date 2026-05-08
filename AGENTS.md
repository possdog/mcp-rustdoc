# mcp-rustdoc

## Build & Test

```bash
cargo build --release          # compile
cargo clippy                   # lint
RUST_LOG=debug cargo run       # run server with visible logs
```

Logging goes to **stderr** — stdout carries the MCP protocol.

## Architecture

```
src/main.rs          entrypoint: tokio runtime + MCP stdio transport
src/server.rs        RustdocServer (manual tool routing, no rmcp macros)
src/render.rs        crate JSON → LLM-friendly Markdown (~1300 lines)
src/fetcher.rs       downloads .json.zst from docs.rs
src/resolver.rs      version resolution: explicit → Cargo.lock → Cargo.toml → docs.rs
src/local.rs         cargo +nightly rustdoc for workspace crates
src/cache.rs         immutable filesystem cache (dirs::cache_dir())
```

## Server: manual tool routing (no rmcp macros)

`RustdocServer` implements `rmcp::ServerHandler` by manually defining
`list_tools` and `call_tool`. This is intentional — it allows
`json_schema_for()` to strip `"null"` from schemars-generated schemas
(Gemini/Vertex AI rejects `type: ["string", "null"]` and `anyOf` with null).

**Do not** switch to `#[tool]`/`#[tool_router]` macros without retesting
against strict schema clients.

## Render pipeline

`src/render.rs` converts `rustdoc_types::Crate` into Markdown. Two layers:

1. **Free functions** — recursive type-level formatters:
   - `format_type` — central function; handles every `Type` variant including
     generics, lifetimes, `impl Trait`, `dyn Trait`, arrays, function pointers,
     and qualified paths.
   - `format_generic_args`, `format_generic_bounds`, `format_generics`,
     `format_where_clause`, `format_fn_signature`, `format_self_param`.

2. **`Render` struct methods** — produce complete output:
   - `list_items`, `get_item`, `search`, `implementations`, `implementors`.

### Key gotchas

- **Lifetime strings** in rustdoc JSON already include the leading `'`
  (e.g. `"'static"`, `"'a"`). Never prepend an extra `'`.
- **`format_type` is the single choke point** for type rendering. Any change
  to how a type variant renders must go through this function.
- **Source links** in `get_item` are built by `make_source_ref()` which
  handles three path formats: relative (`src/foo.rs` → docs.rs URL),
  absolute `/opt/rustwide/…` (→ dep crate docs.rs URL), and other absolute
  paths (→ plain `file:line` for local workspace use).

### `implementations` tool behavior

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

- **No HTTP/SSE** — transport is stdio-only. Do not add other transports
  without explicit request.
- **Markdown output only** — all tools return Markdown strings. Never HTML
  or raw JSON.
- **Local crates require nightly** — `cargo +nightly rustdoc` is needed for
  `--output-format json -Z unstable-options`.
- **Version resolution is baked in** — every tool that takes `crate_name`
  resolves versions internally (explicit version → Cargo.lock → Cargo.toml
  → docs.rs latest). No separate tool call needed.
