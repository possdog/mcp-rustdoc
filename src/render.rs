use fuzzy_matcher::FuzzyMatcher;
use fuzzy_matcher::skim::SkimMatcherV2;
use rustdoc_types::{Crate, Item, ItemEnum};

fn format_type(ty: &rustdoc_types::Type) -> String {
    use rustdoc_types::Type;
    match ty {
        Type::ResolvedPath(p) => p.path.clone(),
        Type::Primitive(p) => p.clone(),
        Type::BorrowedRef {
            is_mutable, type_, ..
        } => {
            let mut_str = if *is_mutable { "mut " } else { "" };
            format!("&{}{}", mut_str, format_type(type_))
        }
        Type::Tuple(tys) => {
            let types: Vec<_> = tys.iter().map(format_type).collect();
            format!("({})", types.join(", "))
        }
        Type::Slice(t) => format!("[{}]", format_type(t)),
        Type::Generic(g) => g.clone(),
        Type::ImplTrait(_) => "impl Trait".to_string(),
        Type::DynTrait(_) => "dyn Trait".to_string(),
        Type::Infer => "_".to_string(),
        Type::RawPointer { is_mutable, type_ } => {
            let mut_str = if *is_mutable { "mut " } else { "const " };
            format!("*{}{}", mut_str, format_type(type_))
        }
        _ => "/* type */".to_string(),
    }
}

pub struct Render;

impl Render {
    pub fn new() -> Self {
        Self
    }

    /// Helper to find an item by its fully qualified path.
    /// This is a simplified lookup. A full implementation would traverse the module tree.
    fn find_item_by_path<'a>(krate: &'a Crate, path: &str) -> Option<&'a Item> {
        // rustdoc_types includes `paths` which maps Id to ItemSummary (path, kind, etc.)
        // We can find the Id that matches the requested path, then look it up in `index`.
        for (id, summary) in &krate.paths {
            let item_path = summary.path.join("::");
            if item_path == path {
                return krate.index.get(id);
            }
        }

        // Fallback: search the index directly for names (less accurate for full paths)
        let parts: Vec<&str> = path.split("::").collect();
        if let Some(last) = parts.last() {
            for item in krate.index.values() {
                if item.name.as_deref() == Some(*last) {
                    // Very naive check, in a real implementation we would verify the full path.
                    return Some(item);
                }
            }
        }

        None
    }

    pub fn list_items(&self, krate: &Crate, path: Option<&str>) -> String {
        let mut output = String::new();

        let root_id = &krate.root;
        let mut target_module = krate.index.get(root_id);

        if let Some(p) = path {
            target_module = Self::find_item_by_path(krate, p);
        }

        if let Some(item) = target_module {
            output.push_str(&format!(
                "# Items in `{}`\n\n",
                item.name.as_deref().unwrap_or("crate root")
            ));

            if let ItemEnum::Module(module) = &item.inner {
                for child_id in &module.items {
                    if let Some(child) = krate.index.get(child_id) {
                        if let Some(name) = &child.name {
                            let kind = match &child.inner {
                                ItemEnum::Module(_) => "module",
                                ItemEnum::Struct(_) => "struct",
                                ItemEnum::Enum(_) => "enum",
                                ItemEnum::Function(_) => "function",
                                ItemEnum::Trait(_) => "trait",
                                ItemEnum::TypeAlias(_) => "type alias",
                                _ => "item",
                            };
                            let summary = child
                                .docs
                                .as_deref()
                                .unwrap_or("")
                                .lines()
                                .next()
                                .unwrap_or("");
                            output.push_str(&format!("- **{}** `{}`: {}\n", kind, name, summary));
                        }
                    }
                }
            } else {
                output.push_str("Provided path is not a module.\n");
            }
        } else {
            output.push_str("Path not found.\n");
        }

        output
    }

    fn resolve_links(krate: &Crate, item: &Item, docs: &str) -> String {
        let mut resolved_docs = docs.to_string();
        for (link_text, id) in &item.links {
            if let Some(summary) = krate.paths.get(id) {
                let full_path = summary.path.join("::");
                // Naive replacement: replace `[link_text]` with `[full_path]`
                // A true markdown parser would be safer, but this covers 90% of rustdoc links
                resolved_docs =
                    resolved_docs.replace(&format!("[{}]", link_text), &format!("[{}]", full_path));
            }
        }
        resolved_docs
    }

    pub fn get_item(&self, krate: &Crate, path: &str) -> String {
        if let Some(item) = Self::find_item_by_path(krate, path) {
            let mut output = String::new();

            let name = item.name.as_deref().unwrap_or("Unknown");
            let kind = match &item.inner {
                ItemEnum::Struct(_) => "Struct",
                ItemEnum::Enum(_) => "Enum",
                ItemEnum::Function(_) => "Function",
                ItemEnum::Trait(_) => "Trait",
                ItemEnum::Module(_) => "Module",
                _ => "Item",
            };

            output.push_str(&format!("# {} `{}`\n\n", kind, name));

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
                output.push_str("\n");
            }

            if let Some(docs) = &item.docs {
                output.push_str(&Self::resolve_links(krate, item, docs));
                output.push_str("\n\n");
            } else {
                output.push_str("*No documentation available.*\n\n");
            }

            // In a full implementation, we would format the struct fields, enum variants, and fn signatures here.
            match &item.inner {
                ItemEnum::Struct(s) => {
                    if let rustdoc_types::StructKind::Plain { fields, .. } = &s.kind {
                        if !fields.is_empty() {
                            output.push_str("### Fields\n\n");
                            for field_id in fields {
                                if let Some(field_item) = krate.index.get(field_id) {
                                    if let Some(name) = &field_item.name {
                                        let ty_str =
                                            if let ItemEnum::StructField(ty) = &field_item.inner {
                                                format!(": {}", format_type(ty))
                                            } else {
                                                String::new()
                                            };
                                        let docs = field_item
                                            .docs
                                            .as_deref()
                                            .unwrap_or("")
                                            .lines()
                                            .next()
                                            .unwrap_or("");
                                        output.push_str(&format!(
                                            "- `{}`{} {}\n",
                                            name, ty_str, docs
                                        ));
                                    }
                                }
                            }
                            output.push_str("\n");
                        }
                    }
                }
                ItemEnum::Enum(e) => {
                    if !e.variants.is_empty() {
                        output.push_str("### Variants\n\n");
                        for var_id in &e.variants {
                            if let Some(var_item) = krate.index.get(var_id) {
                                if let Some(name) = &var_item.name {
                                    let docs = var_item
                                        .docs
                                        .as_deref()
                                        .unwrap_or("")
                                        .lines()
                                        .next()
                                        .unwrap_or("");
                                    output.push_str(&format!("- `{}`: {}\n", name, docs));
                                }
                            }
                        }
                        output.push_str("\n");
                    }
                }
                ItemEnum::Function(f) => {
                    output.push_str("### Signature\n\n```rust\n");
                    output.push_str("fn ");
                    output.push_str(name);
                    output.push_str("(");
                    for (i, (arg_name, arg_type)) in f.sig.inputs.iter().enumerate() {
                        if i > 0 {
                            output.push_str(", ");
                        }
                        output.push_str(&format!("{}: {}", arg_name, format_type(arg_type)));
                    }
                    output.push_str(")");
                    if let Some(ret_type) = &f.sig.output {
                        output.push_str(&format!(" -> {}", format_type(ret_type)));
                    }
                    output.push_str("\n```\n\n");
                }
                _ => {}
            }

            output
        } else {
            format!("Item `{}` not found.", path)
        }
    }

    pub fn search(&self, krate: &Crate, query: &str, limit: usize) -> String {
        let matcher = SkimMatcherV2::default();
        let mut results = Vec::new();

        let terms: Vec<&str> = query.split_whitespace().collect();

        for item in krate.index.values() {
            let mut total_score = 0;
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

        // Sort descending by score
        results.sort_by(|a, b| b.0.cmp(&a.0));

        let mut output = format!("# Search Results for `{}`\n\n", query);
        for (_, item) in results.into_iter().take(limit) {
            let name = item.name.as_deref().unwrap_or("Unknown");
            let kind = match &item.inner {
                ItemEnum::Module(_) => "module",
                ItemEnum::Struct(_) => "struct",
                ItemEnum::Enum(_) => "enum",
                ItemEnum::Function(_) => "function",
                ItemEnum::Trait(_) => "trait",
                _ => "item",
            };
            let summary = item
                .docs
                .as_deref()
                .unwrap_or("")
                .lines()
                .next()
                .unwrap_or("");
            output.push_str(&format!("- **{}** `{}`: {}\n", kind, name, summary));
        }

        if output.ends_with("\n\n") {
            output.push_str("*No results found.*\n");
        }

        output
    }

    pub fn implementations(&self, krate: &Crate, path: &str) -> String {
        if let Some(item) = Self::find_item_by_path(krate, path) {
            let impls = match &item.inner {
                ItemEnum::Struct(s) => &s.impls,
                ItemEnum::Enum(e) => &e.impls,
                ItemEnum::Union(u) => &u.impls,
                ItemEnum::Trait(t) => &t.implementations,
                _ => return format!("Item `{}` does not have implementations.", path),
            };

            let mut output = format!("# Implementations for `{}`\n\n", path);
            if impls.is_empty() {
                output.push_str("*No implementations found.*\n");
                return output;
            }

            for id in impls {
                if let Some(impl_item) = krate.index.get(id) {
                    if let ItemEnum::Impl(imp) = &impl_item.inner {
                        if let Some(trait_) = &imp.trait_ {
                            output.push_str(&format!("- `impl {} for {}`\n", trait_.path, path));
                        } else {
                            output.push_str(&format!("- `impl {}`\n", path));
                        }
                    }
                }
            }
            output
        } else {
            format!("Item `{}` not found.", path)
        }
    }

    pub fn implementors(&self, krate: &Crate, path: &str) -> String {
        if let Some(item) = Self::find_item_by_path(krate, path) {
            let impls = match &item.inner {
                ItemEnum::Trait(t) => &t.implementations,
                _ => return format!("Item `{}` is not a trait.", path),
            };

            let mut output = format!("# Implementors of `{}`\n\n", path);
            if impls.is_empty() {
                output.push_str("*No implementors found.*\n");
                return output;
            }

            for id in impls {
                if let Some(impl_item) = krate.index.get(id) {
                    if let ItemEnum::Impl(imp) = &impl_item.inner {
                        let for_name = match &imp.for_ {
                            rustdoc_types::Type::ResolvedPath(p) => p.path.clone(),
                            _ => "Unknown Type".to_string(),
                        };
                        output.push_str(&format!("- `{}`\n", for_name));
                    }
                }
            }
            output
        } else {
            format!("Item `{}` not found.", path)
        }
    }
}
