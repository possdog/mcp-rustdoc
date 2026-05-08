use fuzzy_matcher::FuzzyMatcher;
use fuzzy_matcher::skim::SkimMatcherV2;
use rustdoc_types::{
    Abi, AssocItemConstraintKind, Crate, GenericArg, GenericArgs, GenericBound, GenericParamDefKind,
    Item, ItemEnum, Term, TraitBoundModifier, WherePredicate,
};

// ---------------------------------------------------------------------------
// Type formatting helpers
// ---------------------------------------------------------------------------

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
                            Term::Constant(c) => {
                                c.value.clone().unwrap_or_else(|| c.expr.clone())
                            }
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
                Some(format!(
                    "{}{}{}{}",
                    hrtb, modifier_str, trait_.path, args
                ))
            }
            GenericBound::Outlives(l) => Some(l.clone()), // already includes leading '
            GenericBound::Use(_) => None, // precise capturing — skip
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
            let my_args = args
                .as_deref()
                .map(format_generic_args)
                .unwrap_or_default();
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
            if let Type::Generic(g) = type_.as_ref() {
                if g == "Self" {
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
            if krate.index.get(id).map(|i| std::ptr::eq(i, item)).unwrap_or(false) {
                return Some(summary.path.join("::"));
            }
        }
        None
    }

    fn resolve_links(krate: &Crate, item: &Item, docs: &str) -> String {
        let mut resolved = docs.to_string();
        for (link_text, id) in &item.links {
            if let Some(summary) = krate.paths.get(id) {
                let full_path = summary.path.join("::");
                resolved = resolved.replace(
                    &format!("[{}]", link_text),
                    &format!("[{}]", full_path),
                );
            }
        }
        resolved
    }

    // -----------------------------------------------------------------------
    // list_items
    // -----------------------------------------------------------------------

    pub fn list_items(&self, krate: &Crate, path: Option<&str>) -> String {
        let root_id = &krate.root;
        let target_module = if let Some(p) = path {
            Self::find_item_by_path(krate, p)
        } else {
            krate.index.get(root_id)
        };

        let Some(item) = target_module else {
            return "Path not found.\n".to_string();
        };

        let ItemEnum::Module(module) = &item.inner else {
            return "Provided path is not a module.\n".to_string();
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
                    let target_kind = u
                        .id
                        .as_ref()
                        .and_then(|id| krate.index.get(id))
                        .map(|t| Self::item_kind_str(&t.inner))
                        .unwrap_or("item");
                    let target_docs = u
                        .id
                        .as_ref()
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
                    (
                        format!("{}::*", u.source),
                        "use (glob)",
                        String::new(),
                    )
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

            output.push_str(&format!(
                "- **{}** `{}`: {}\n",
                kind, display_name, summary
            ));
        }

        output
    }

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

    pub fn get_item(&self, krate: &Crate, path: &str) -> String {
        let Some(item) = Self::find_item_by_path(krate, path) else {
            return format!("Item `{}` not found.", path);
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
                output.push_str(&format!(
                    "# Function `{}`\n\n",
                    name
                ));
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

        // --- Feature gates ---
        let mut has_features = false;
        for attr in &item.attrs {
            if let rustdoc_types::Attribute::Other(s) = attr {
                if s.contains("cfg(") || s.contains("feature =") {
                    if !has_features {
                        output.push_str("**Feature Gates:**\n");
                        has_features = true;
                    }
                    output.push_str(&format!("- `{}`\n", s));
                }
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
            ItemEnum::Struct(s) => {
                match &s.kind {
                    rustdoc_types::StructKind::Plain { fields, .. } => {
                        if !fields.is_empty() {
                            output.push_str("### Fields\n\n");
                            for field_id in fields {
                                if let Some(field_item) = krate.index.get(field_id) {
                                    if let Some(fname) = &field_item.name {
                                        let ty_str =
                                            if let ItemEnum::StructField(ty) = &field_item.inner {
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
                                        output.push_str(&format!(
                                            "- `{}`{} {}\n",
                                            fname, ty_str, doc
                                        ));
                                    }
                                }
                            }
                            output.push('\n');
                        }
                    }
                    rustdoc_types::StructKind::Tuple(fields) => {
                        let types: Vec<String> = fields
                            .iter()
                            .filter_map(|opt_id| {
                                opt_id.as_ref().and_then(|id| krate.index.get(id))
                            })
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
                }
            }

            ItemEnum::Enum(e) => {
                if !e.variants.is_empty() {
                    output.push_str("### Variants\n\n");
                    for var_id in &e.variants {
                        if let Some(var_item) = krate.index.get(var_id) {
                            if let Some(vname) = &var_item.name {
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
                let val = const_
                    .value
                    .as_deref()
                    .unwrap_or_else(|| const_.expr.as_str());
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

        output
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
                ItemEnum::AssocType { generics, bounds, type_ } => {
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
                    let entry = format!("type {}{}{}{};", child_name, gen_str, bounds_str, default_str);
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

        results.sort_by(|a, b| b.0.cmp(&a.0));

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

    pub fn implementations(&self, krate: &Crate, path: &str) -> String {
        let Some(item) = Self::find_item_by_path(krate, path) else {
            return format!("Item `{}` not found.", path);
        };

        match &item.inner {
            // For traits: show the trait's own items (what callers must implement)
            ItemEnum::Trait(t) => {
                let mut output = format!("# Interface of trait `{}`\n\n", path);
                Self::render_trait_items(krate, t, &mut output);
                output
            }

            // For structs/enums/unions: list impl blocks
            ItemEnum::Struct(s) => Self::render_type_impls(krate, path, &s.impls),
            ItemEnum::Enum(e) => Self::render_type_impls(krate, path, &e.impls),
            ItemEnum::Union(u) => Self::render_type_impls(krate, path, &u.impls),

            _ => format!("Item `{}` does not have implementations.", path),
        }
    }

    fn render_type_impls(
        krate: &Crate,
        path: &str,
        impls: &[rustdoc_types::Id],
    ) -> String {
        let mut output = format!("# Implementations for `{}`\n\n", path);
        if impls.is_empty() {
            output.push_str("*No implementations found.*\n");
            return output;
        }

        for id in impls {
            let Some(impl_item) = krate.index.get(id) else {
                continue;
            };
            let ItemEnum::Impl(imp) = &impl_item.inner else {
                continue;
            };

            // Skip synthetic/compiler-generated impls to reduce noise
            if imp.is_synthetic {
                continue;
            }

            let for_type = format_type(&imp.for_);

            if let Some(trait_) = &imp.trait_ {
                let trait_args = trait_
                    .args
                    .as_deref()
                    .map(format_generic_args)
                    .unwrap_or_default();
                let neg = if imp.is_negative { "!" } else { "" };
                output.push_str(&format!(
                    "- `impl {}{}{} for {}`",
                    neg, trait_.path, trait_args, for_type
                ));
            } else {
                // Inherent impl: list method names
                let method_names: Vec<&str> = imp
                    .items
                    .iter()
                    .filter_map(|mid| krate.index.get(mid))
                    .filter_map(|m| m.name.as_deref())
                    .collect();
                output.push_str(&format!("- `impl {}`", for_type));
                if !method_names.is_empty() {
                    output.push_str(&format!(": `{}`", method_names.join("`, `")));
                }
            }
            output.push('\n');
        }
        output
    }

    // -----------------------------------------------------------------------
    // implementors
    // -----------------------------------------------------------------------

    pub fn implementors(&self, krate: &Crate, path: &str) -> String {
        let Some(item) = Self::find_item_by_path(krate, path) else {
            return format!("Item `{}` not found.", path);
        };

        let ItemEnum::Trait(t) = &item.inner else {
            return format!("Item `{}` is not a trait.", path);
        };

        let mut output = format!("# Implementors of `{}`\n\n", path);
        if t.implementations.is_empty() {
            output.push_str("*No implementors found in this crate.*\n");
            return output;
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
        output
    }
}
