//! Converts `rustdoc_types::Crate` structs into LLM-optimized Markdown.
//!
//! ## Architecture
//!
//! The rendering pipeline has two layers:
//!
//! 1. **Free functions** — recursive type-level formatters that operate on
//!    individual rustdoc-types data structures (`format_type`,
//!    `format_generic_args`, `format_generic_bounds`, `format_generics`,
//!    `format_where_clause`, `format_fn_signature`, etc.).
//!
//! 2. **`Render` struct methods** — higher-level entry points that consume
//!    a full `Crate` and produce complete Markdown output for each subcommand
//!    (`list-items`, `get-item`, `search`, `implementations`, `implementors`).
//!
//! The key design constraint is that all output is plain Markdown — never
//! HTML, never raw JSON trees. Type signatures are rendered as Rust code
//! blocks, and output is kept concise to minimize LLM context usage.
//!
//! ## Known limitations
//!
//! * `find_item_by_path` falls back to a last-segment name match when the
//!   full path is not in `krate.paths`. This is non-deterministic when items
//!   in different modules share the same name. In practice this only affects
//!   private/internal items not surfaced in `krate.paths`.
//!
//! * `resolve_links` does not handle `[text][id]` reference-style links or
//!   namespace-prefixed links (`[struct@Foo]`).
//!
//! * Lifetime strings in rustdoc JSON already include the leading `'`, so
//!   formatters must not prepend an extra one.

use crate::error::{CratedocError, Result};
use fuzzy_matcher::FuzzyMatcher;
use fuzzy_matcher::skim::SkimMatcherV2;
use rustdoc_types::{
    Abi, AssocItemConstraintKind, Crate, GenericArg, GenericArgs, GenericBound,
    GenericParamDefKind, Item, ItemEnum, Term, TraitBoundModifier, WherePredicate,
};

// ---------------------------------------------------------------------------
// Type formatting helpers
// ---------------------------------------------------------------------------

/// Format a set of generic arguments — angle-bracketed (`<T, 'a>`),
/// parenthesized (`(A, B) -> C`), or return-type notation (`(..)`).
///
/// Handles associated item constraints such as `Item = u32` and
/// `IntoIter: Clone`.
fn format_generic_args(args: &GenericArgs) -> String {
    match args {
        GenericArgs::AngleBracketed { args, constraints } => {
            let mut parts: Vec<String> = Vec::new();
            for arg in args {
                match arg {
                    GenericArg::Lifetime(l) => parts.push(l.clone()), // already includes leading '
                    GenericArg::Type(t) => parts.push(format_type(t)),
                    GenericArg::Const(c) => {
                        parts.push(c.value.clone().unwrap_or_else(|| c.expr.clone()))
                    }
                    GenericArg::Infer => parts.push("_".to_string()),
                }
            }
            for constraint in constraints {
                let args_str = constraint
                    .args
                    .as_deref()
                    .map(format_generic_args)
                    .unwrap_or_default();
                let binding = match &constraint.binding {
                    AssocItemConstraintKind::Equality(term) => {
                        let rhs = match term {
                            Term::Type(t) => format_type(t),
                            Term::Constant(c) => c.value.clone().unwrap_or_else(|| c.expr.clone()),
                        };
                        format!("{}{} = {}", constraint.name, args_str, rhs)
                    }
                    AssocItemConstraintKind::Constraint(bounds) => {
                        format!(
                            "{}{}: {}",
                            constraint.name,
                            args_str,
                            format_generic_bounds(bounds)
                        )
                    }
                };
                parts.push(binding);
            }
            if parts.is_empty() {
                String::new()
            } else {
                format!("<{}>", parts.join(", "))
            }
        }
        GenericArgs::Parenthesized { inputs, output } => {
            let inputs_str: Vec<String> = inputs.iter().map(format_type).collect();
            let output_str = output
                .as_ref()
                .map(|t| format!(" -> {}", format_type(t)))
                .unwrap_or_default();
            format!("({}){}", inputs_str.join(", "), output_str)
        }
        GenericArgs::ReturnTypeNotation => "(..)".to_string(),
    }
}

/// Format a list of generic bounds, joined with ` + `.
///
/// Handles trait bounds (`Clone + Send`), HRTBs (`for<'a> Fn(&'a str)`),
/// trait bound modifiers (`?Sized`, `~const`), and lifetime outlives bounds
/// (`'a: 'b + 'c`). Precise-capturing `use<…>` bounds are skipped — they
/// have no surface-level syntax in stable Rust.
fn format_generic_bounds(bounds: &[GenericBound]) -> String {
    let parts: Vec<String> = bounds
        .iter()
        .filter_map(|b| match b {
            GenericBound::TraitBound {
                trait_,
                generic_params,
                modifier,
            } => {
                let modifier_str = match modifier {
                    TraitBoundModifier::None => "",
                    TraitBoundModifier::Maybe => "?",
                    TraitBoundModifier::MaybeConst => "~const ",
                };
                // Higher-Rank Trait Bounds: `for<'a> Fn(&'a str)`
                let hrtb = if !generic_params.is_empty() {
                    let lifetimes: Vec<String> = generic_params
                        .iter()
                        .filter_map(|p| {
                            if matches!(&p.kind, GenericParamDefKind::Lifetime { .. }) {
                                Some(p.name.clone()) // already includes leading '
                            } else {
                                None
                            }
                        })
                        .collect();
                    if lifetimes.is_empty() {
                        String::new()
                    } else {
                        format!("for<{}> ", lifetimes.join(", "))
                    }
                } else {
                    String::new()
                };
                let args = trait_
                    .args
                    .as_deref()
                    .map(format_generic_args)
                    .unwrap_or_default();
                Some(format!("{}{}{}{}", hrtb, modifier_str, trait_.path, args))
            }
            GenericBound::Outlives(l) => Some(l.clone()), // already includes leading '
            GenericBound::Use(_) => None,                 // precise capturing — skip
        })
        .collect();
    parts.join(" + ")
}

/// Format the generic parameter list for an item header, e.g. `<T: Clone, U>`.
fn format_generics(generics: &rustdoc_types::Generics) -> String {
    let params: Vec<String> = generics
        .params
        .iter()
        .filter_map(|p| match &p.kind {
            GenericParamDefKind::Lifetime { .. } => Some(p.name.clone()), // already includes leading '
            GenericParamDefKind::Type {
                bounds,
                is_synthetic,
                ..
            } => {
                if *is_synthetic {
                    return None;
                }
                if bounds.is_empty() {
                    Some(p.name.clone())
                } else {
                    Some(format!("{}: {}", p.name, format_generic_bounds(bounds)))
                }
            }
            GenericParamDefKind::Const { type_, .. } => {
                Some(format!("const {}: {}", p.name, format_type(type_)))
            }
        })
        .collect();
    if params.is_empty() {
        String::new()
    } else {
        format!("<{}>", params.join(", "))
    }
}

/// Format a `where` clause, e.g. `\nwhere\n    T: Clone,\n    U: Copy`.
fn format_where_clause(generics: &rustdoc_types::Generics) -> String {
    if generics.where_predicates.is_empty() {
        return String::new();
    }
    let predicates: Vec<String> = generics
        .where_predicates
        .iter()
        .map(|p| match p {
            WherePredicate::BoundPredicate { type_, bounds, .. } => {
                format!("{}: {}", format_type(type_), format_generic_bounds(bounds))
            }
            WherePredicate::LifetimePredicate { lifetime, outlives } => {
                // lifetime strings already include the leading '
                if outlives.is_empty() {
                    lifetime.clone()
                } else {
                    format!("{}: {}", lifetime, outlives.join(" + "))
                }
            }
            WherePredicate::EqPredicate { lhs, rhs } => {
                let rhs_str = match rhs {
                    Term::Type(t) => format_type(t),
                    Term::Constant(c) => c.value.clone().unwrap_or_else(|| c.expr.clone()),
                };
                format!("{} = {}", format_type(lhs), rhs_str)
            }
        })
        .collect();
    format!("\nwhere\n    {}", predicates.join(",\n    "))
}

/// Format where predicates as a compact comma-separated inline string (no newlines).
/// Used for impl block headers like `impl Foo<T> where T: Clone`.
fn format_where_predicates_inline(generics: &rustdoc_types::Generics) -> String {
    if generics.where_predicates.is_empty() {
        return String::new();
    }
    let predicates: Vec<String> = generics
        .where_predicates
        .iter()
        .map(|p| match p {
            WherePredicate::BoundPredicate { type_, bounds, .. } => {
                format!("{}: {}", format_type(type_), format_generic_bounds(bounds))
            }
            WherePredicate::LifetimePredicate { lifetime, outlives } => {
                if outlives.is_empty() {
                    lifetime.clone()
                } else {
                    format!("{}: {}", lifetime, outlives.join(" + "))
                }
            }
            WherePredicate::EqPredicate { lhs, rhs } => {
                let rhs_str = match rhs {
                    Term::Type(t) => format_type(t),
                    Term::Constant(c) => c.value.clone().unwrap_or_else(|| c.expr.clone()),
                };
                format!("{} = {}", format_type(lhs), rhs_str)
            }
        })
        .collect();
    predicates.join(", ")
}

// ---------------------------------------------------------------------------
// Source link helpers
// ---------------------------------------------------------------------------

/// Parse a registry package directory like `axum-core-0.5.5` into
/// `("axum-core", "0.5.5")`. The version always starts with a digit
/// after a hyphen, which disambiguates from hyphenated crate names.
fn parse_pkg_dir(dir: &str) -> Option<(&str, &str)> {
    let bytes = dir.as_bytes();
    for i in 1..bytes.len() {
        if bytes[i - 1] == b'-' && bytes[i].is_ascii_digit() {
            return Some((&dir[..i - 1], &dir[i..]));
        }
    }
    None
}

/// Build a Markdown source reference string for an item span.
///
/// - Relative paths (`src/foo.rs`) → docs.rs URL for the queried crate.
/// - Absolute `/opt/rustwide/…` paths → docs.rs URL for the dep crate.
/// - Other absolute paths (local workspace) → plain `file:line` text.
fn make_source_ref(filename: &str, line: usize, crate_name: &str, version: &str) -> String {
    const RUSTWIDE_PREFIX: &str = "/opt/rustwide/cargo-home/registry/src/";

    // Convert relative `src/foo/bar.rs` to the URL path component
    // docs.rs uses: src/{crate_name}/foo/bar.rs.html#{line}
    let docs_rs_url = |name: &str, ver: &str, src_relative: &str| -> String {
        // src_relative is like "src/foo/bar.rs" — strip "src/" then reattach under crate name
        let inner = src_relative.strip_prefix("src/").unwrap_or(src_relative);
        let name_under = name.replace('-', "_");
        format!(
            "https://docs.rs/{}/{}/src/{}/{}.html#{}",
            name, ver, name_under, inner, line
        )
    };

    if let Some(rest) = filename.strip_prefix(RUSTWIDE_PREFIX) {
        // Absolute docs.rs build path:
        // {registry_id}/{dep_name}-{dep_version}/src/...
        if let Some(slash) = rest.find('/') {
            let pkg_path = &rest[slash + 1..]; // "{name}-{version}/src/..."
            if let Some(slash2) = pkg_path.find('/') {
                let pkg_dir = &pkg_path[..slash2]; // "{name}-{version}"
                let rel_path = &pkg_path[slash2 + 1..]; // "src/..."
                if let Some((dep_name, dep_ver)) = parse_pkg_dir(pkg_dir) {
                    let url = docs_rs_url(dep_name, dep_ver, rel_path);
                    return format!("[`{}:{}`]({})", rel_path, line, url);
                }
            }
        }
        // Couldn't parse — fall through to plain text
        return format!("`{}:{}`", filename, line);
    }

    if filename.starts_with('/') {
        // Local workspace file — show as plain path:line for agent's file tools
        return format!("`{}:{}`", filename, line);
    }

    // Relative path — the crate's own source
    if !crate_name.is_empty() && !version.is_empty() {
        let url = docs_rs_url(crate_name, version, filename);
        format!("[`{}:{}`]({})", filename, line, url)
    } else {
        format!("`{}:{}`", filename, line)
    }
}

/// Recursively format a `rustdoc_types::Type` into its Rust source-code
/// representation.
///
/// This is the central type-rendering function. It handles every variant of
/// `Type` in the rustdoc-types schema, including resolved paths with generic
/// arguments, references with lifetimes, trait objects, function pointers,
/// qualified paths (`<T as Trait>::Name`), arrays, slices, tuples, and raw
/// pointers. Unknown/rare variants fall back to `/* type */`.
fn format_type(ty: &rustdoc_types::Type) -> String {
    use rustdoc_types::Type;
    match ty {
        Type::ResolvedPath(p) => {
            let args = p
                .args
                .as_deref()
                .map(format_generic_args)
                .unwrap_or_default();
            format!("{}{}", p.path, args)
        }
        Type::Primitive(p) => p.clone(),
        Type::BorrowedRef {
            lifetime,
            is_mutable,
            type_,
        } => {
            // Lifetime strings already include the leading ', e.g. "'static", "'a"
            let lt = lifetime
                .as_deref()
                .map(|l| format!("{} ", l))
                .unwrap_or_default();
            let mut_str = if *is_mutable { "mut " } else { "" };
            format!("&{}{}{}", lt, mut_str, format_type(type_))
        }
        Type::Tuple(tys) => {
            let types: Vec<_> = tys.iter().map(format_type).collect();
            format!("({})", types.join(", "))
        }
        Type::Slice(t) => format!("[{}]", format_type(t)),
        Type::Array { type_, len } => format!("[{}; {}]", format_type(type_), len),
        Type::Generic(g) => g.clone(),
        Type::ImplTrait(bounds) => format!("impl {}", format_generic_bounds(bounds)),
        Type::DynTrait(dyn_trait) => {
            let mut parts: Vec<String> = dyn_trait
                .traits
                .iter()
                .map(|pt| {
                    let args = pt
                        .trait_
                        .args
                        .as_deref()
                        .map(format_generic_args)
                        .unwrap_or_default();
                    format!("{}{}", pt.trait_.path, args)
                })
                .collect();
            if let Some(lt) = &dyn_trait.lifetime {
                parts.push(lt.clone()); // already includes leading '
            }
            format!("dyn {}", parts.join(" + "))
        }
        Type::Infer => "_".to_string(),
        Type::RawPointer { is_mutable, type_ } => {
            let mut_str = if *is_mutable { "mut " } else { "const " };
            format!("*{}{}", mut_str, format_type(type_))
        }
        Type::FunctionPointer(fp) => {
            let inputs: Vec<String> = fp
                .sig
                .inputs
                .iter()
                .map(|(_, ty)| format_type(ty))
                .collect();
            let output = fp
                .sig
                .output
                .as_ref()
                .map(|t| format!(" -> {}", format_type(t)))
                .unwrap_or_default();
            let unsafe_str = if fp.header.is_unsafe { "unsafe " } else { "" };
            let abi_str = match &fp.header.abi {
                Abi::C { .. } => "extern \"C\" ".to_string(),
                Abi::Other(s) => format!("extern \"{}\" ", s),
                _ => String::new(),
            };
            format!(
                "{}{}fn({}){}",
                unsafe_str,
                abi_str,
                inputs.join(", "),
                output
            )
        }
        Type::QualifiedPath {
            name,
            args,
            self_type,
            trait_,
        } => {
            let my_args = args.as_deref().map(format_generic_args).unwrap_or_default();
            match trait_ {
                Some(tr) if !tr.path.is_empty() => {
                    // Fully qualified: <Self as Trait>::Name
                    let trait_args = tr
                        .args
                        .as_deref()
                        .map(format_generic_args)
                        .unwrap_or_default();
                    format!(
                        "<{} as {}{}>::{}{}",
                        format_type(self_type),
                        tr.path,
                        trait_args,
                        name,
                        my_args
                    )
                }
                _ => {
                    // Trait path is absent or empty — render as a simple
                    // associated type access, e.g. `S::Ok`
                    format!("{}::{}{}", format_type(self_type), name, my_args)
                }
            }
        }
        _ => "/* type */".to_string(),
    }
}

/// Render a method receiver (`self`, `&self`, `&mut self`, or `self: Type`)
/// from the rustdoc JSON representation.
fn format_self_param(ty: &rustdoc_types::Type) -> String {
    use rustdoc_types::Type;
    match ty {
        Type::Generic(g) if g == "Self" => "self".to_string(),
        Type::BorrowedRef {
            lifetime,
            is_mutable,
            type_,
        } => {
            if let Type::Generic(g) = type_.as_ref()
                && g == "Self"
            {
                let lt = lifetime
                    .as_deref()
                    .map(|l| format!("{} ", l))
                    .unwrap_or_default();
                return if *is_mutable {
                    format!("&{}mut self", lt)
                } else {
                    format!("&{}self", lt)
                };
            }
            // Non-Self borrowed ref receiver (rare); fall through to generic form
            format!("self: {}", format_type(ty))
        }
        _ => format!("self: {}", format_type(ty)),
    }
}

/// Build a complete function signature string (without trailing newline).
fn format_fn_signature(name: &str, f: &rustdoc_types::Function) -> String {
    let mut sig = String::new();
    if f.header.is_const {
        sig.push_str("const ");
    }
    if f.header.is_async {
        sig.push_str("async ");
    }
    if f.header.is_unsafe {
        sig.push_str("unsafe ");
    }
    match &f.header.abi {
        Abi::C { .. } => sig.push_str("extern \"C\" "),
        Abi::Cdecl { .. } => sig.push_str("extern \"cdecl\" "),
        Abi::Stdcall { .. } => sig.push_str("extern \"stdcall\" "),
        Abi::Fastcall { .. } => sig.push_str("extern \"fastcall\" "),
        Abi::Aapcs { .. } => sig.push_str("extern \"aapcs\" "),
        Abi::Win64 { .. } => sig.push_str("extern \"win64\" "),
        Abi::SysV64 { .. } => sig.push_str("extern \"sysv64\" "),
        Abi::System { .. } => sig.push_str("extern \"system\" "),
        Abi::Other(s) => {
            sig.push_str("extern \"");
            sig.push_str(s);
            sig.push_str("\" ");
        }
        Abi::Rust => {}
    }
    sig.push_str("fn ");
    sig.push_str(name);
    sig.push_str(&format_generics(&f.generics));
    sig.push('(');
    let args: Vec<String> = f
        .sig
        .inputs
        .iter()
        .map(|(arg_name, ty)| {
            if arg_name == "self" {
                format_self_param(ty)
            } else {
                format!("{}: {}", arg_name, format_type(ty))
            }
        })
        .collect();
    sig.push_str(&args.join(", "));
    if f.sig.is_c_variadic {
        if !args.is_empty() {
            sig.push_str(", ");
        }
        sig.push_str("...");
    }
    sig.push(')');
    if let Some(ret) = &f.sig.output {
        sig.push_str(" -> ");
        sig.push_str(&format_type(ret));
    }
    let where_clause = format_where_clause(&f.generics);
    if !where_clause.is_empty() {
        sig.push_str(&where_clause);
    }
    sig
}

// ---------------------------------------------------------------------------
// Render
// ---------------------------------------------------------------------------

pub struct Render;

impl Default for Render {
    fn default() -> Self {
        Self::new()
    }
}

impl Render {
    pub fn new() -> Self {
        Self
    }

    /// Find an item by its fully-qualified path, consulting `krate.paths` first
    /// and falling back to a name-based scan of `krate.index`.
    fn find_item_by_path<'a>(krate: &'a Crate, path: &str) -> Option<&'a Item> {
        for (id, summary) in &krate.paths {
            if summary.path.join("::") == path {
                return krate.index.get(id);
            }
        }
        // Fallback: last path segment name match (non-deterministic when names collide)
        let last = path.split("::").last()?;
        krate
            .index
            .values()
            .find(|item| item.name.as_deref() == Some(last))
    }

    /// Return the fully-qualified path for an item if it appears in `krate.paths`.
    fn item_path(krate: &Crate, item: &Item) -> Option<String> {
        // The item's Id must be in krate.paths for us to find its path.
        // We look for any entry whose Id matches item's id.
        // Item.id is not directly accessible as a field, so we invert krate.paths.
        for (id, summary) in &krate.paths {
            if krate
                .index
                .get(id)
                .map(|i| std::ptr::eq(i, item))
                .unwrap_or(false)
            {
                return Some(summary.path.join("::"));
            }
        }
        None
    }

    /// Naively resolve `[link_text]` references in rustdoc doc comments to
    /// their fully-qualified path strings. Links using the `[text][id]`
    /// reference-style syntax are not resolved.
    fn resolve_links(krate: &Crate, item: &Item, docs: &str) -> String {
        let mut resolved = docs.to_string();
        for (link_text, id) in &item.links {
            if let Some(summary) = krate.paths.get(id) {
                let full_path = summary.path.join("::");
                resolved =
                    resolved.replace(&format!("[{}]", link_text), &format!("[{}]", full_path));
            }
        }
        resolved
    }

    // -----------------------------------------------------------------------
    // list-items
    // -----------------------------------------------------------------------

    /// Build a Markdown listing of all direct children of a module.
    ///
    /// When `path` is `None`, lists the crate root. When `path` is `Some`,
    /// looks up the named module and lists its contents. Re-exports (`use`
    /// items) are resolved to show the target item's kind and doc summary.
    pub fn list_items(&self, krate: &Crate, path: Option<&str>) -> Result<String> {
        let root_id = &krate.root;
        let target_module = if let Some(p) = path {
            Self::find_item_by_path(krate, p)
        } else {
            krate.index.get(root_id)
        };

        let Some(item) = target_module else {
            return Err(CratedocError::Other(format!(
                "Path `{}` not found.",
                path.unwrap_or("")
            )));
        };

        let ItemEnum::Module(module) = &item.inner else {
            return Err(CratedocError::Other(format!(
                "Path `{}` is not a module.",
                path.unwrap_or("")
            )));
        };

        let mut output = format!(
            "# Items in `{}`\n\n",
            item.name.as_deref().unwrap_or("crate root")
        );

        for child_id in &module.items {
            let Some(child) = krate.index.get(child_id) else {
                continue;
            };

            // For Use re-exports, resolve the target item so we can show its kind
            let (display_name, kind, summary) = match &child.inner {
                ItemEnum::Use(u) if !u.is_glob => {
                    // Try to get the kind from the target item
                    let target_kind =
                        u.id.as_ref()
                            .and_then(|id| krate.index.get(id))
                            .map(|t| Self::item_kind_str(&t.inner))
                            .unwrap_or("item");
                    let target_docs =
                        u.id.as_ref()
                            .and_then(|id| krate.index.get(id))
                            .and_then(|t| t.docs.as_deref())
                            .unwrap_or("")
                            .lines()
                            .next()
                            .unwrap_or("");
                    (u.name.clone(), target_kind, target_docs.to_string())
                }
                ItemEnum::Use(u) if u.is_glob => {
                    // Glob imports: e.g. `pub use foo::*`
                    (format!("{}::*", u.source), "use (glob)", String::new())
                }
                _ => {
                    let Some(name) = &child.name else { continue };
                    let kind = Self::item_kind_str(&child.inner);
                    let doc = child
                        .docs
                        .as_deref()
                        .unwrap_or("")
                        .lines()
                        .next()
                        .unwrap_or("")
                        .to_string();
                    (name.clone(), kind, doc)
                }
            };

            output.push_str(&format!("- **{}** `{}`: {}\n", kind, display_name, summary));
        }

        Ok(output)
    }

    /// Resolve an `ItemEnum` variant to a human-readable kind label for
    /// use in `list-items` and `search` output.
    fn item_kind_str(inner: &ItemEnum) -> &'static str {
        match inner {
            ItemEnum::Module(_) => "module",
            ItemEnum::Struct(_) => "struct",
            ItemEnum::Enum(_) => "enum",
            ItemEnum::Union(_) => "union",
            ItemEnum::Function(_) => "function",
            ItemEnum::Trait(_) => "trait",
            ItemEnum::TraitAlias(_) => "trait alias",
            ItemEnum::TypeAlias(_) => "type alias",
            ItemEnum::Constant { .. } => "constant",
            ItemEnum::Static(_) => "static",
            ItemEnum::Macro(_) => "macro",
            ItemEnum::ProcMacro(_) => "proc macro",
            _ => "item",
        }
    }

    // -----------------------------------------------------------------------
    // get_item
    // -----------------------------------------------------------------------

    /// Build a detailed Markdown page for a single item — struct, enum,
    /// function, trait, module, type alias, constant, or static.
    ///
    /// Output includes: header with generics, source link (docs.rs or local),
    /// feature gates, deprecation notice, full doc comment with resolved
    /// links, and kind-specific sections (fields, variants, signature,
    /// trait items, type alias definition, constant/static definition).
    pub fn get_item(&self, krate: &Crate, path: &str) -> Result<String> {
        let Some(item) = Self::find_item_by_path(krate, path) else {
            return Err(CratedocError::Other(format!("Item `{}` not found.", path)));
        };

        let mut output = String::new();
        let name = item.name.as_deref().unwrap_or("Unknown");

        // --- Header line ---
        match &item.inner {
            ItemEnum::Struct(s) => {
                let generics = format_generics(&s.generics);
                let where_clause = format_where_clause(&s.generics);
                output.push_str(&format!(
                    "# Struct `{}{}`{}\n\n",
                    name, generics, where_clause
                ));
            }
            ItemEnum::Enum(e) => {
                let generics = format_generics(&e.generics);
                let where_clause = format_where_clause(&e.generics);
                output.push_str(&format!(
                    "# Enum `{}{}`{}\n\n",
                    name, generics, where_clause
                ));
            }
            ItemEnum::Trait(t) => {
                let generics = format_generics(&t.generics);
                let where_clause = format_where_clause(&t.generics);
                let unsafe_str = if t.is_unsafe { "unsafe " } else { "" };
                output.push_str(&format!(
                    "# {}Trait `{}{}`{}\n\n",
                    unsafe_str, name, generics, where_clause
                ));
            }
            ItemEnum::Function(f) => {
                output.push_str(&format!("# Function `{}`\n\n", name));
                let _ = f; // will be handled below in the match
            }
            ItemEnum::TypeAlias(ta) => {
                let generics = format_generics(&ta.generics);
                output.push_str(&format!("# Type Alias `{}{}`\n\n", name, generics));
            }
            ItemEnum::Constant { .. } => {
                output.push_str(&format!("# Constant `{}`\n\n", name));
            }
            ItemEnum::Static(s) => {
                let mut_str = if s.is_mutable { "mut " } else { "" };
                output.push_str(&format!("# Static `{}`\n\n", name));
                let _ = mut_str;
            }
            ItemEnum::Module(_) => {
                output.push_str(&format!("# Module `{}`\n\n", name));
            }
            _ => {
                output.push_str(&format!("# Item `{}`\n\n", name));
            }
        }

        // --- Source link ---
        if let Some(span) = &item.span {
            let crate_name = krate
                .index
                .get(&krate.root)
                .and_then(|r| r.name.as_deref())
                .unwrap_or("");
            let version = krate.crate_version.as_deref().unwrap_or("");
            let src_ref = make_source_ref(
                &span.filename.to_string_lossy(),
                span.begin.0,
                crate_name,
                version,
            );
            output.push_str(&format!("*Source: {}*\n\n", src_ref));
        }

        // --- Feature gates ---
        let mut has_features = false;
        for attr in &item.attrs {
            if let rustdoc_types::Attribute::Other(s) = attr
                && (s.contains("cfg(") || s.contains("feature ="))
            {
                if !has_features {
                    output.push_str("**Feature Gates:**\n");
                    has_features = true;
                }
                output.push_str(&format!("- `{}`\n", s));
            }
        }
        if has_features {
            output.push('\n');
        }

        // --- Deprecation ---
        if let Some(dep) = &item.deprecation {
            output.push_str("**Deprecated**");
            if let Some(since) = &dep.since {
                output.push_str(&format!(" since {}", since));
            }
            if let Some(note) = &dep.note {
                output.push_str(&format!(": {}", note));
            }
            output.push_str("\n\n");
        }

        // --- Doc comment ---
        if let Some(docs) = &item.docs {
            output.push_str(&Self::resolve_links(krate, item, docs));
            output.push_str("\n\n");
        } else {
            output.push_str("*No documentation available.*\n\n");
        }

        // --- Kind-specific sections ---
        match &item.inner {
            ItemEnum::Struct(s) => match &s.kind {
                rustdoc_types::StructKind::Plain { fields, .. } => {
                    if !fields.is_empty() {
                        output.push_str("### Fields\n\n");
                        for field_id in fields {
                            if let Some(field_item) = krate.index.get(field_id)
                                && let Some(fname) = &field_item.name
                            {
                                let ty_str = if let ItemEnum::StructField(ty) = &field_item.inner {
                                    format!(": {}", format_type(ty))
                                } else {
                                    String::new()
                                };
                                let doc = field_item
                                    .docs
                                    .as_deref()
                                    .unwrap_or("")
                                    .lines()
                                    .next()
                                    .unwrap_or("");
                                output.push_str(&format!("- `{}`{} {}\n", fname, ty_str, doc));
                            }
                        }
                        output.push('\n');
                    }
                }
                rustdoc_types::StructKind::Tuple(fields) => {
                    let types: Vec<String> = fields
                        .iter()
                        .filter_map(|opt_id| opt_id.as_ref().and_then(|id| krate.index.get(id)))
                        .map(|field_item| {
                            if let ItemEnum::StructField(ty) = &field_item.inner {
                                format_type(ty)
                            } else {
                                "?".to_string()
                            }
                        })
                        .collect();
                    if !types.is_empty() {
                        output.push_str(&format!(
                            "### Fields\n\nTuple struct: `{}({})`\n\n",
                            name,
                            types.join(", ")
                        ));
                    }
                }
                rustdoc_types::StructKind::Unit => {
                    output.push_str("### Fields\n\nUnit struct (no fields).\n\n");
                }
            },

            ItemEnum::Enum(e) => {
                if !e.variants.is_empty() {
                    output.push_str("### Variants\n\n");
                    for var_id in &e.variants {
                        if let Some(var_item) = krate.index.get(var_id)
                            && let Some(vname) = &var_item.name
                        {
                            let doc = var_item
                                .docs
                                .as_deref()
                                .unwrap_or("")
                                .lines()
                                .next()
                                .unwrap_or("");

                            if let ItemEnum::Variant(v) = &var_item.inner {
                                let variant_sig = match &v.kind {
                                    rustdoc_types::VariantKind::Plain => vname.clone(),
                                    rustdoc_types::VariantKind::Tuple(fields) => {
                                        let types: Vec<String> = fields
                                            .iter()
                                            .filter_map(|opt_id| {
                                                opt_id.as_ref().and_then(|id| krate.index.get(id))
                                            })
                                            .map(|fi| {
                                                if let ItemEnum::StructField(ty) = &fi.inner {
                                                    format_type(ty)
                                                } else {
                                                    "?".to_string()
                                                }
                                            })
                                            .collect();
                                        format!("{}({})", vname, types.join(", "))
                                    }
                                    rustdoc_types::VariantKind::Struct { fields, .. } => {
                                        let field_strs: Vec<String> = fields
                                            .iter()
                                            .filter_map(|id| krate.index.get(id))
                                            .filter_map(|fi| {
                                                fi.name.as_ref().map(|fn_| {
                                                    if let ItemEnum::StructField(ty) = &fi.inner {
                                                        format!("{}: {}", fn_, format_type(ty))
                                                    } else {
                                                        fn_.clone()
                                                    }
                                                })
                                            })
                                            .collect();
                                        format!("{} {{ {} }}", vname, field_strs.join(", "))
                                    }
                                };
                                output.push_str(&format!("- `{}`: {}\n", variant_sig, doc));
                            } else {
                                output.push_str(&format!("- `{}`: {}\n", vname, doc));
                            }
                        }
                    }
                    output.push('\n');
                }
            }

            ItemEnum::Function(f) => {
                output.push_str("### Signature\n\n```rust\n");
                output.push_str(&format_fn_signature(name, f));
                output.push_str("\n```\n\n");
            }

            ItemEnum::Trait(t) => {
                Self::render_trait_items(krate, t, &mut output);
            }

            ItemEnum::TypeAlias(ta) => {
                output.push_str(&format!(
                    "### Definition\n\n```rust\ntype {}{} = {}{}\n```\n\n",
                    name,
                    format_generics(&ta.generics),
                    format_type(&ta.type_),
                    format_where_clause(&ta.generics),
                ));
            }

            ItemEnum::Constant { type_, const_ } => {
                let val = const_.value.as_deref().unwrap_or(const_.expr.as_str());
                output.push_str(&format!(
                    "### Definition\n\n```rust\nconst {}: {} = {};\n```\n\n",
                    name,
                    format_type(type_),
                    val
                ));
            }

            ItemEnum::Static(s) => {
                let mut_str = if s.is_mutable { "mut " } else { "" };
                output.push_str(&format!(
                    "### Definition\n\n```rust\nstatic {}{}: {};\n```\n\n",
                    mut_str,
                    name,
                    format_type(&s.type_)
                ));
            }

            _ => {}
        }

        Ok(output)
    }

    /// Render required methods, provided methods, associated types, and
    /// associated constants for a trait item.
    fn render_trait_items(krate: &Crate, t: &rustdoc_types::Trait, output: &mut String) {
        let mut required_methods: Vec<String> = Vec::new();
        let mut provided_methods: Vec<String> = Vec::new();
        let mut assoc_types: Vec<String> = Vec::new();
        let mut assoc_consts: Vec<String> = Vec::new();

        for id in &t.items {
            let Some(child) = krate.index.get(id) else {
                continue;
            };
            let child_name = child.name.as_deref().unwrap_or("_");
            let child_doc = child
                .docs
                .as_deref()
                .unwrap_or("")
                .lines()
                .next()
                .unwrap_or("");

            match &child.inner {
                ItemEnum::Function(f) => {
                    let sig = format_fn_signature(child_name, f);
                    let entry = if child_doc.is_empty() {
                        format!("```rust\n{}\n```", sig)
                    } else {
                        format!("```rust\n{}\n```\n  *{}*", sig, child_doc)
                    };
                    if f.has_body {
                        provided_methods.push(entry);
                    } else {
                        required_methods.push(entry);
                    }
                }
                ItemEnum::AssocType {
                    generics,
                    bounds,
                    type_,
                } => {
                    let gen_str = format_generics(generics);
                    let bounds_str = if !bounds.is_empty() {
                        format!(": {}", format_generic_bounds(bounds))
                    } else {
                        String::new()
                    };
                    let default_str = type_
                        .as_ref()
                        .map(|t| format!(" = {}", format_type(t)))
                        .unwrap_or_default();
                    let entry = format!(
                        "type {}{}{}{};",
                        child_name, gen_str, bounds_str, default_str
                    );
                    assoc_types.push(if child_doc.is_empty() {
                        format!("`{}`", entry)
                    } else {
                        format!("`{}` — {}", entry, child_doc)
                    });
                }
                ItemEnum::AssocConst { type_, value } => {
                    let val_str = value
                        .as_deref()
                        .map(|v| format!(" = {}", v))
                        .unwrap_or_default();
                    let entry = format!("const {}: {}{};", child_name, format_type(type_), val_str);
                    assoc_consts.push(if child_doc.is_empty() {
                        format!("`{}`", entry)
                    } else {
                        format!("`{}` — {}", entry, child_doc)
                    });
                }
                _ => {}
            }
        }

        if !assoc_types.is_empty() {
            output.push_str("### Associated Types\n\n");
            for entry in &assoc_types {
                output.push_str(&format!("- {}\n", entry));
            }
            output.push('\n');
        }

        if !assoc_consts.is_empty() {
            output.push_str("### Associated Constants\n\n");
            for entry in &assoc_consts {
                output.push_str(&format!("- {}\n", entry));
            }
            output.push('\n');
        }

        if !required_methods.is_empty() {
            output.push_str("### Required Methods\n\n");
            for entry in &required_methods {
                output.push_str(entry);
                output.push_str("\n\n");
            }
        }

        if !provided_methods.is_empty() {
            output.push_str("### Provided Methods\n\n");
            for entry in &provided_methods {
                output.push_str(entry);
                output.push_str("\n\n");
            }
        }
    }

    // -----------------------------------------------------------------------
    // search
    // -----------------------------------------------------------------------

    /// Fuzzy-search item names and doc comments across the entire crate,
    /// returning up to `limit` results sorted by match score.
    ///
    /// Uses the `skim` fuzzy-matcher (SkimV2 algorithm) with multi-word
    /// matching: every whitespace-separated query term must fuzzy-match
    /// either the item name or its doc comment. Doc comment matches count
    /// at half weight. Results include the full qualified path for context.
    pub fn search(&self, krate: &Crate, query: &str, limit: usize) -> String {
        let matcher = SkimMatcherV2::default();
        let terms: Vec<&str> = query.split_whitespace().collect();
        let mut results: Vec<(i64, &Item)> = Vec::new();

        for item in krate.index.values() {
            let mut total_score = 0i64;
            let mut matched_all = true;

            for term in &terms {
                let name_score = item
                    .name
                    .as_deref()
                    .and_then(|n| matcher.fuzzy_match(n, term));
                let doc_score = item
                    .docs
                    .as_deref()
                    .and_then(|d| matcher.fuzzy_match(d, term))
                    .map(|s| s / 2);

                if let Some(score) = name_score.or(doc_score) {
                    total_score += score;
                } else {
                    matched_all = false;
                    break;
                }
            }

            if matched_all && total_score > 0 {
                results.push((total_score, item));
            }
        }

        results.sort_by_key(|(score, _)| std::cmp::Reverse(*score));

        let mut output = format!("# Search Results for `{}`\n\n", query);
        for (_, item) in results.into_iter().take(limit) {
            let name = item.name.as_deref().unwrap_or("Unknown");
            let kind = Self::item_kind_str(&item.inner);

            // Try to show the full path for context
            let path_str = Self::item_path(krate, item)
                .map(|p| format!("`{}`", p))
                .unwrap_or_else(|| format!("`{}`", name));

            let summary = item
                .docs
                .as_deref()
                .unwrap_or("")
                .lines()
                .next()
                .unwrap_or("");
            output.push_str(&format!("- **{}** {}: {}\n", kind, path_str, summary));
        }

        if output.ends_with("\n\n") {
            output.push_str("*No results found.*\n");
        }

        output
    }

    // -----------------------------------------------------------------------
    // implementations
    // -----------------------------------------------------------------------

    /// For a struct, enum, or union: list all impl blocks (inherent methods
    /// shown with full signatures grouped by impl block; trait implementations
    /// collected into a compact list).
    ///
    /// For a trait: show the trait's own interface — required methods,
    /// provided methods, associated types, and associated constants.
    pub fn implementations(&self, krate: &Crate, path: &str) -> Result<String> {
        let Some(item) = Self::find_item_by_path(krate, path) else {
            return Err(CratedocError::Other(format!("Item `{}` not found.", path)));
        };

        match &item.inner {
            // For traits: show the trait's own items (what callers must implement)
            ItemEnum::Trait(t) => {
                let mut output = format!("# Interface of trait `{}`\n\n", path);
                Self::render_trait_items(krate, t, &mut output);
                Ok(output)
            }

            // For structs/enums/unions: list impl blocks
            ItemEnum::Struct(s) => Ok(Self::render_type_impls(krate, path, &s.impls)),
            ItemEnum::Enum(e) => Ok(Self::render_type_impls(krate, path, &e.impls)),
            ItemEnum::Union(u) => Ok(Self::render_type_impls(krate, path, &u.impls)),

            _ => Err(CratedocError::Other(format!(
                "Item `{}` does not have implementations.",
                path
            ))),
        }
    }

    /// Render impl blocks for a concrete type.
    ///
    /// Separate passes:
    /// 1. The complete list of trait impl names for a given type — `Clone for Router<S>`, `Debug for Router<S>`, etc.
    /// 2. The full signature of each inherent method organized by impl block followed by a compact list of trait implementations.
    fn render_type_impls(krate: &Crate, path: &str, impls: &[rustdoc_types::Id]) -> String {
        let mut output = format!("# Implementations for `{}`\n\n", path);
        if impls.is_empty() {
            output.push_str("*No implementations found.*\n");
            return output;
        }

        let mut trait_impl_strs: Vec<String> = Vec::new();
        let mut found_any = false;

        for id in impls {
            let Some(impl_item) = krate.index.get(id) else {
                continue;
            };
            let ItemEnum::Impl(imp) = &impl_item.inner else {
                continue;
            };

            if imp.is_synthetic {
                continue;
            }

            let for_type = format_type(&imp.for_);

            if let Some(trait_) = &imp.trait_ {
                // Collect trait impls for a compact list at the end
                let trait_args = trait_
                    .args
                    .as_deref()
                    .map(format_generic_args)
                    .unwrap_or_default();
                let neg = if imp.is_negative { "!" } else { "" };
                trait_impl_strs.push(format!(
                    "`{}{}{} for {}`",
                    neg, trait_.path, trait_args, for_type
                ));
            } else {
                // Inherent impl block: render a section with full method signatures
                found_any = true;

                let where_clause = format_where_predicates_inline(&imp.generics);
                if where_clause.is_empty() {
                    output.push_str(&format!("**`impl {}`**\n\n", for_type));
                } else {
                    output.push_str(&format!(
                        "**`impl {} where {}`**\n\n",
                        for_type, where_clause
                    ));
                }

                let mut sigs: Vec<String> = Vec::new();
                for mid in &imp.items {
                    if let Some(method_item) = krate.index.get(mid)
                        && let ItemEnum::Function(f) = &method_item.inner
                    {
                        let method_name = method_item.name.as_deref().unwrap_or("_");
                        sigs.push(format_fn_signature(method_name, f));
                    }
                }

                if sigs.is_empty() {
                    output.push_str("*No public methods.*\n\n");
                } else {
                    output.push_str("```rust\n");
                    for sig in &sigs {
                        output.push_str(sig);
                        output.push('\n');
                    }
                    output.push_str("```\n\n");
                }
            }
        }

        // Trait implementations — compact bulleted list
        if !trait_impl_strs.is_empty() {
            found_any = true;
            output.push_str("**Trait implementations:**\n\n");
            for s in &trait_impl_strs {
                output.push_str(&format!("- {}\n", s));
            }
            output.push('\n');
        }

        if !found_any {
            output.push_str("*No implementations found.*\n");
        }

        output
    }

    // -----------------------------------------------------------------------
    // implementors
    // -----------------------------------------------------------------------

    /// For a trait: list every concrete type *within this crate* that
    /// implements it. Types are formatted via `format_type` so generics,
    /// arrays, primitives, and tuple types all render correctly.
    ///
    /// Cross-crate implementors (e.g. `impl Iterator for MyType` where
    /// `MyType` lives in a different crate) are an architectural limitation
    /// of rustdoc JSON — the JSON only contains impls defined within the
    /// queried crate.
    pub fn implementors(&self, krate: &Crate, path: &str) -> Result<String> {
        let Some(item) = Self::find_item_by_path(krate, path) else {
            return Err(CratedocError::Other(format!("Item `{}` not found.", path)));
        };

        let ItemEnum::Trait(t) = &item.inner else {
            return Err(CratedocError::Other(format!(
                "Item `{}` is not a trait.",
                path
            )));
        };

        let mut output = format!("# Implementors of `{}`\n\n", path);
        if t.implementations.is_empty() {
            output.push_str("*No implementors found in this crate.*\n");
            return Ok(output);
        }

        for id in &t.implementations {
            let Some(impl_item) = krate.index.get(id) else {
                continue;
            };
            let ItemEnum::Impl(imp) = &impl_item.inner else {
                continue;
            };
            output.push_str(&format!("- `{}`\n", format_type(&imp.for_)));
        }
        Ok(output)
    }
}
