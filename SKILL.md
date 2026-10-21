---
name: rustdoc
description: Load when you need Rust crate documentation — API signatures, item details, impl blocks, or trait implementors for any crate on docs.rs or in the local workspace. Provides usage for the `mcp-rustdoc` CLI, which renders rustdoc JSON as Markdown.
---

# rustdoc (mcp-rustdoc CLI)

`mcp-rustdoc` renders rustdoc JSON (docs.rs or local workspace) as Markdown on
stdout. Version resolution is automatic: explicit `--version` → local
workspace crate → `Cargo.lock` → `Cargo.toml` → crates.io latest. No version
lookup step is needed before querying a crate the current project depends on.

## Subcommands

```bash
mcp-rustdoc list-workspace-crates                # crates in the current workspace
mcp-rustdoc list-items <crate> [module-path]     # items in a crate or module
mcp-rustdoc get-item <crate> <item-path>         # full docs for one item
mcp-rustdoc search <crate> <query> [--limit N]   # fuzzy search names + docs
mcp-rustdoc implementations <crate> <type-path>  # impl blocks for a type
mcp-rustdoc implementors <crate> <trait-path>     # types implementing a trait
```

All crate-taking subcommands accept `--version <VERSION>` to pin a release.

## Typical workflow

1. Unsure what exists? `search <crate> <keywords>`, or `list-items <crate>`
   for a crate overview.
2. Explore a module: `list-items <crate> <module-path>`.
3. Drill into an item: `get-item <crate> <full::path::Item>` — includes doc
   comments, signatures, fields, variants, feature gates, and source links.
4. Type methods: `implementations`; who implements a trait:
   `implementors`.

Item paths are fully qualified (`serde::de::Deserialize`), not bare names.

## Piping

Output is Markdown on stdout; logs and errors go to stderr. Not-found items
exit 1, so results compose with shell tools:

```bash
mcp-rustdoc list-items tokio | grep -i runtime
mcp-rustdoc search serde error --limit 20 | head -40
mcp-rustdoc get-item serde serde::Serialize | head -60
```

Prefer piping through `head`/`grep` when you only need part of the output —
some crates (e.g. `serde`) produce very large pages.

## Gotchas

- Local workspace crates are preferred automatically when no `--version` is
  given; generating them requires the nightly toolchain
  (`rustup toolchain install nightly`).
- docs.rs only serves rustdoc JSON for crates published after 2025-05-23.
  Older versions 404 — there is no workaround.
- `implementors` only lists implementors defined in the queried crate
  (rustdoc JSON has no cross-crate impl data).
- Fetched crates are cached on disk forever (immutable per version); a first
  query for a large crate may take a few seconds, later ones are instant.
