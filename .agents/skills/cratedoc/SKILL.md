---
name: cratedoc
description: Load when a task needs Rust crate documentation — API signatures, item details, impl blocks, trait implementors, or exploring an unfamiliar crate. Provides exact usage of the `cratedoc` CLI, including version resolution, caching, and failure modes.
---

# cratedoc

`cratedoc` renders rustdoc JSON as Markdown on stdout. Sources:

- **Published crates** — fetched from docs.rs (and cached on disk).
- **Local workspace crates** — generated on the fly with
  `cargo +nightly rustdoc` when the crate is a member of the current
  workspace.

## Subcommands

```
cratedoc list-items <crate> [module-path] [--version <VERSION>]
cratedoc get-item <crate> <item-path> [--version <VERSION>]
cratedoc search <crate> <query> [--limit <N>] [--version <VERSION>]
cratedoc implementations <crate> <type-path> [--version <VERSION>]
cratedoc implementors <crate> <trait-path> [--version <VERSION>]
cratedoc list-workspace-crates
```

`search` defaults to `--limit 10`. `--version` pins an exact crate version;
without it, resolution is (in order):

1. **Local workspace member** — if the crate is in the current workspace
   (requires the nightly toolchain: `rustup toolchain install nightly`).
2. **`Cargo.lock`** — the exact pinned version, searching upward from the CWD.
3. **`Cargo.toml`** — the dependency spec with `^`/`~`/`=` sigils stripped.
4. **crates.io** — latest published version.

Passing `--version` skips the local-workspace path entirely and fetches that
exact version from docs.rs (or the cache). Note that when invoked inside a
Rust project, dependency queries resolve to the *project's pinned version*,
not the latest release.

## Typical workflow

1. `search <crate> <keywords>` to find items when names are uncertain.
2. `list-items <crate>` for a crate overview; `list-items <crate> <module>`
   to explore one module.
3. `get-item <crate> <full::path::Item>` for docs, signatures, fields,
   variants, feature gates, and a source link.
4. `implementations` for a type's method surface; `implementors` for who
   implements a trait.

## Item paths

Use fully-qualified paths (`serde::de::Deserialize`). Paths are matched
exactly against rustdoc's path table first; a bare or partial name works only
via a last-`::`-segment fallback, which is ambiguous when items share a name
across modules. The paths shown by `search` and `list-items` are safe to feed
to `get-item`.

## Output, exit codes, piping

- Markdown on stdout; logs and errors on stderr.
- Success: exit 0. Runtime errors (item not found, path is not a module,
  item is not a trait, crate not on crates.io, docs.rs 404): exit 1 with the
  message on stderr. Argument misuse: exit 2.

```bash
cratedoc list-items tokio | grep -i runtime
cratedoc search serde error --limit 20 | head -40
cratedoc get-item serde serde::Serialize | head -60
cratedoc get-item serde serde::Nope || echo "not found"
```

Pipe through `head` when you only need part of the output — large crates
produce very large pages. Output is generated JSON-docs rendered to text, so
it is grep-friendly but not line-stable across versions.

## Caching

- Published crates are cached permanently at
  `{system cache dir}/cratedoc/{crate}/{version}/`
  (macOS: `~/Library/Caches/cratedoc/`, Linux: `~/.cache/cratedoc/`).
  Entries are immutable per crate+version; delete the directory to force a
  re-fetch. A first query for a large uncached crate can take seconds.
- Local workspace docs are generated once into
  `{target-dir}/doc/{crate_name_with_underscores}.json` and regenerated only
  when `Cargo.toml` or files under `src/` are newer than that JSON.

## Gotchas and limits

- **docs.rs age limit** — rustdoc JSON exists only for crate versions
  published after 2025-05-23; older versions 404 with no workaround.
- **Cross-crate implementors** — `implementors` and `implementations` only
  see impl blocks defined in the queried crate; a trait implemented for a
  type in another crate will not appear.
- **`implementations` on a trait** shows the trait's own interface
  (required/provided methods, associated types/constants) — that is the
  trait contract, not its implementors; use `implementors` for those.
- **`search`** is fuzzy multi-word matching (skim algorithm): every
  whitespace-separated term must match the item name or its doc text; doc
  matches score lower. Results show kind, qualified path, and first doc
  line, sorted best-first.
- **`list-workspace-crates`** lists workspace members (name, version,
  manifest path) and errors outside a cargo project.
