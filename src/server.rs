use crate::cache::Cache;
use crate::error::McpRustdocError;
use crate::fetcher::Fetcher;
use crate::local::Local;
use crate::render::Render;
use crate::resolver::Resolver;

use rmcp::{
    RoleServer, ServerHandler,
    model::{
        CallToolRequestParams, CallToolResult, Content, ErrorCode, ErrorData, Implementation,
        InitializeRequestParams, InitializeResult, ListToolsResult, PaginatedRequestParams,
        ProtocolVersion, ServerCapabilities, ServerInfo, Tool, ToolsCapability,
    },
    service::RequestContext,
};
use rustdoc_types::Crate;
use schemars::schema_for;

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct ListItemsParams {
    pub crate_name: String,
    pub version: Option<String>,
    pub item_path: Option<String>,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct GetItemParams {
    pub crate_name: String,
    pub version: Option<String>,
    pub item_path: String,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct SearchParams {
    pub crate_name: String,
    pub version: Option<String>,
    pub query: String,
    pub limit: Option<usize>,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct ImplementationsParams {
    pub crate_name: String,
    pub version: Option<String>,
    pub item_path: String,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct ImplementorsParams {
    pub crate_name: String,
    pub version: Option<String>,
    pub trait_path: String,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct ListWorkspaceCratesParams {}

/// Generates JSON Schema for a type with a workaround for Gemini/Vertex AI
/// strict schema requirements.
///
/// `schemars` emits `type: ["string", "null"]` and
/// `anyOf: [{type: "string"}, {type: "null"}]` for `Option<T>` fields.
/// Some strict schema consumers (Gemini/Vertex AI tool calling) reject both
/// forms. This function strips `"null"` from any such type unions. The field
/// remains optional because it is not in the `required` array of the parent
/// schema object.
fn json_schema_for<T: schemars::JsonSchema>() -> serde_json::Map<String, serde_json::Value> {
    let schema = schema_for!(T);
    let mut value = serde_json::to_value(&schema).unwrap();

    // Workaround for Gemini/Vertex AI strict schema requirements
    // They reject `type: ["string", "null"]` which schemars emits for Option<T>.
    // They also reject `anyOf: [{type: "string"}, {type: "null"}]`.
    // We strip the "null" type out. The field remains optional because it is not in the `required` array.
    if let Some(obj) = value.as_object_mut()
        && let Some(props) = obj.get_mut("properties").and_then(|p| p.as_object_mut())
    {
        for (_, prop) in props.iter_mut() {
            if let Some(prop_obj) = prop.as_object_mut() {
                // Check if it has an "anyOf" field containing a null type
                if let Some(any_of) = prop_obj.get("anyOf").and_then(|a| a.as_array()) {
                    let has_null = any_of.iter().any(|v| {
                        v.as_object()
                            .and_then(|o| o.get("type"))
                            .and_then(|t| t.as_str())
                            == Some("null")
                    });

                    if has_null {
                        // Extract the non-null object
                        if let Some(non_null_obj) = any_of.iter().find(|v| {
                            v.as_object()
                                .and_then(|o| o.get("type"))
                                .and_then(|t| t.as_str())
                                != Some("null")
                        }) {
                            // We replace the `anyOf` field entirely with the contents of the non-null object
                            let new_type = non_null_obj.get("type").unwrap().clone();
                            prop_obj.insert("type".to_string(), new_type);
                            prop_obj.remove("anyOf");
                        }
                    }
                }

                // Check if it has a direct "type" array
                if let Some(types) = prop_obj.get("type").and_then(|t| t.as_array())
                    && types.iter().any(|v| v == "null")
                {
                    // Extract the non-null type (e.g. "string" or "integer")
                    if let Some(non_null_type) = types.iter().find(|v| *v != "null") {
                        let new_type = non_null_type.clone();
                        prop_obj.insert("type".to_string(), new_type);
                    }
                }
            }
        }
    }

    value.as_object().unwrap().clone()
}

/// The MCP server implementation — holds the component instances
/// (cache, fetcher, local, resolver, render) behind `Arc`s for sharing.
#[derive(Clone)]
pub struct RustdocServer {
    cache: std::sync::Arc<Cache>,
    fetcher: std::sync::Arc<Fetcher>,
    local: std::sync::Arc<Local>,
    resolver: std::sync::Arc<Resolver>,
    render: std::sync::Arc<Render>,
}

impl RustdocServer {
    pub fn new() -> Result<Self, McpRustdocError> {
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
    /// 2. Resolve version via `Resolver` (Cargo.lock → Cargo.toml → docs.rs latest).
    /// 3. Check disk cache.
    /// 4. Fetch from docs.rs and cache the result.
    async fn get_crate(
        &self,
        crate_name: &str,
        version: Option<&str>,
    ) -> Result<Crate, McpRustdocError> {
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

    fn to_error_data(e: impl ToString) -> ErrorData {
        ErrorData {
            code: ErrorCode(-32603),
            message: e.to_string().into(),
            data: None,
        }
    }

    async fn list_items_impl(
        &self,
        crate_name: &str,
        version: Option<&str>,
        item_path: Option<&str>,
    ) -> Result<CallToolResult, ErrorData> {
        let krate = self
            .get_crate(crate_name, version)
            .await
            .map_err(Self::to_error_data)?;

        let output = self.render.list_items(&krate, item_path);
        Ok(CallToolResult::success(vec![Content::text(output)]))
    }

    async fn get_item_impl(
        &self,
        crate_name: &str,
        version: Option<&str>,
        item_path: &str,
    ) -> Result<CallToolResult, ErrorData> {
        let krate = self
            .get_crate(crate_name, version)
            .await
            .map_err(Self::to_error_data)?;

        let output = self.render.get_item(&krate, item_path);
        Ok(CallToolResult::success(vec![Content::text(output)]))
    }

    async fn search_impl(
        &self,
        crate_name: &str,
        version: Option<&str>,
        query: &str,
        limit: Option<usize>,
    ) -> Result<CallToolResult, ErrorData> {
        let krate = self
            .get_crate(crate_name, version)
            .await
            .map_err(Self::to_error_data)?;

        let limit = limit.unwrap_or(10);
        let output = self.render.search(&krate, query, limit);
        Ok(CallToolResult::success(vec![Content::text(output)]))
    }

    async fn implementations_impl(
        &self,
        crate_name: &str,
        version: Option<&str>,
        item_path: &str,
    ) -> Result<CallToolResult, ErrorData> {
        let krate = self
            .get_crate(crate_name, version)
            .await
            .map_err(Self::to_error_data)?;

        let output = self.render.implementations(&krate, item_path);
        Ok(CallToolResult::success(vec![Content::text(output)]))
    }

    async fn implementors_impl(
        &self,
        crate_name: &str,
        version: Option<&str>,
        trait_path: &str,
    ) -> Result<CallToolResult, ErrorData> {
        let krate = self
            .get_crate(crate_name, version)
            .await
            .map_err(Self::to_error_data)?;

        let output = self.render.implementors(&krate, trait_path);
        Ok(CallToolResult::success(vec![Content::text(output)]))
    }

    async fn list_workspace_crates_impl(&self) -> Result<CallToolResult, ErrorData> {
        let output = self
            .local
            .list_workspace_crates()
            .await
            .map_err(Self::to_error_data)?;
        Ok(CallToolResult::success(vec![Content::text(output)]))
    }
}

impl ServerHandler for RustdocServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            protocol_version: ProtocolVersion::V_2025_06_18,
            capabilities: ServerCapabilities {
                tools: Some(ToolsCapability {
                    list_changed: Some(false),
                }),
                ..Default::default()
            },
            server_info: Implementation {
                name: "mcp-rustdoc".to_string(),
                version: "0.1.0".to_string(),
                title: None,
                website_url: None,
                icons: None,
            },
            instructions: Some("I provide access to Rust crate documentation.".to_string()),
        }
    }

    async fn initialize(
        &self,
        _params: InitializeRequestParams,
        _ctx: RequestContext<RoleServer>,
    ) -> Result<InitializeResult, ErrorData> {
        Ok(self.get_info())
    }

    fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _ctx: RequestContext<RoleServer>,
    ) -> impl std::future::Future<Output = Result<ListToolsResult, ErrorData>> + Send + '_ {
        let list_items_schema = json_schema_for::<ListItemsParams>();
        let get_item_schema = json_schema_for::<GetItemParams>();
        let search_schema = json_schema_for::<SearchParams>();
        let implementations_schema = json_schema_for::<ImplementationsParams>();
        let implementors_schema = json_schema_for::<ImplementorsParams>();
        let list_workspace_crates_schema = json_schema_for::<ListWorkspaceCratesParams>();

        async move {
            Ok(ListToolsResult {
                tools: vec![
                    Tool {
                        name: "list_items".into(),
                        description: Some("List items in a crate or module — structs, enums, functions, traits, modules, constants, type aliases, etc.".into()),
                        input_schema: std::sync::Arc::new(list_items_schema),
                        title: None,
                        annotations: None,
                        output_schema: None,
                        icons: None,
                        meta: None,
                    },
                    Tool {
                        name: "get_item".into(),
                        description: Some("Get detailed documentation for a specific item — includes full doc comments, type signature, struct fields, enum variants, function parameters, feature gates, and code examples.".into()),
                        input_schema: std::sync::Arc::new(get_item_schema),
                        title: None,
                        annotations: None,
                        output_schema: None,
                        icons: None,
                        meta: None,
                    },
                    Tool {
                        name: "search".into(),
                        description: Some("Full-text search across item names and doc comments with fuzzy matching.".into()),
                        input_schema: std::sync::Arc::new(search_schema),
                        title: None,
                        annotations: None,
                        output_schema: None,
                        icons: None,
                        meta: None,
                    },
                    Tool {
                        name: "implementations".into(),
                        description: Some("For a given type, return all of its impl blocks — both inherent methods and trait implementations.".into()),
                        input_schema: std::sync::Arc::new(implementations_schema),
                        title: None,
                        annotations: None,
                        output_schema: None,
                        icons: None,
                        meta: None,
                    },
                    Tool {
                        name: "implementors".into(),
                        description: Some("For a given trait, return all concrete types that implement it.".into()),
                        input_schema: std::sync::Arc::new(implementors_schema),
                        title: None,
                        annotations: None,
                        output_schema: None,
                        icons: None,
                        meta: None,
                    },
                    Tool {
                        name: "list_workspace_crates".into(),
                        description: Some("List all crates in the current workspace with their versions, local paths, and availability status.".into()),
                        input_schema: std::sync::Arc::new(list_workspace_crates_schema),
                        title: None,
                        annotations: None,
                        output_schema: None,
                        icons: None,
                        meta: None,
                    },
                ],
                next_cursor: None,
                meta: None,
            })
        }
    }

    async fn call_tool(
        &self,
        params: CallToolRequestParams,
        _ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let name = params.name.clone();
        let arguments = params.arguments.unwrap_or_default();

        match name.as_ref() {
            "list_items" => {
                let p: ListItemsParams =
                    serde_json::from_value(serde_json::Value::Object(arguments))
                        .map_err(Self::to_error_data)?;
                self.list_items_impl(&p.crate_name, p.version.as_deref(), p.item_path.as_deref())
                    .await
            }
            "get_item" => {
                let p: GetItemParams = serde_json::from_value(serde_json::Value::Object(arguments))
                    .map_err(Self::to_error_data)?;
                self.get_item_impl(&p.crate_name, p.version.as_deref(), &p.item_path)
                    .await
            }
            "search" => {
                let p: SearchParams = serde_json::from_value(serde_json::Value::Object(arguments))
                    .map_err(Self::to_error_data)?;
                self.search_impl(&p.crate_name, p.version.as_deref(), &p.query, p.limit)
                    .await
            }
            "implementations" => {
                let p: ImplementationsParams =
                    serde_json::from_value(serde_json::Value::Object(arguments))
                        .map_err(Self::to_error_data)?;
                self.implementations_impl(&p.crate_name, p.version.as_deref(), &p.item_path)
                    .await
            }
            "implementors" => {
                let p: ImplementorsParams =
                    serde_json::from_value(serde_json::Value::Object(arguments))
                        .map_err(Self::to_error_data)?;
                self.implementors_impl(&p.crate_name, p.version.as_deref(), &p.trait_path)
                    .await
            }
            "list_workspace_crates" => self.list_workspace_crates_impl().await,
            _ => Err(Self::to_error_data(format!("Unknown tool: {}", name))),
        }
    }
}
