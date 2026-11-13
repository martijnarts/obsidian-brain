//! The MCP surface: one handler that serves every vault's tools, each with
//! a required `vault` argument.

pub mod http;
pub mod tool;
mod validate;

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, Implementation,
    ListToolsResult, PaginatedRequestParams, ServerCapabilities, ServerConfig, Tool,
};
use rmcp::service::RequestContext;
use rmcp::{ErrorData as McpError, RoleServer, ServerHandler};
use serde_json::{Value, json};

use crate::context::VaultContext;
use crate::util::js;
use crate::vaults::Vaults;
use tool::{InvalidArguments, JsonObject, ToolSpec};
use validate::ArgsValidator;

pub const SERVER_NAME: &str = "obsidian-brain";

const LIST_VAULTS: &str = "list_vaults";
const LIST_VAULTS_DESCRIPTION: &str = "List the vaults this server serves. Every other tool takes one of these names as its required `vault` argument.";
const VAULT_ARG_DESCRIPTION: &str = "The vault to work in. `list_vaults` describes each vault.";

/// The tool set of every vault, merged. Cheap to clone.
#[derive(Clone)]
pub struct Server {
    vaults: Arc<Vaults>,
    tools: Arc<Vec<ToolSpec>>,
    /// The input schema of each tool in `tools`, with `vault`.
    validators: Arc<Vec<ArgsValidator>>,
    by_name: Arc<BTreeMap<&'static str, usize>>,
}

impl Server {
    pub fn new(vaults: Arc<Vaults>, tools: Vec<ToolSpec>) -> Self {
        let by_name = tools.iter().enumerate().map(|(i, t)| (t.name, i)).collect();
        let validators = tools
            .iter()
            .map(|t| ArgsValidator::new(Value::Object(input_schema(&vaults, t))))
            .collect();
        Self {
            vaults,
            tools: Arc::new(tools),
            validators: Arc::new(validators),
            by_name: Arc::new(by_name),
        }
    }

    /// The tool names, `list_vaults` first.
    pub fn names(&self) -> Vec<&'static str> {
        std::iter::once(LIST_VAULTS)
            .chain(self.tools.iter().map(|t| t.name))
            .collect()
    }

    pub fn tool_list(&self) -> Vec<Tool> {
        let mut out = vec![Tool::new(
            LIST_VAULTS,
            LIST_VAULTS_DESCRIPTION,
            Arc::new(tool::input_schema_for::<tool::NoArgs>()),
        )];
        for spec in self.tools.iter() {
            let schema = input_schema(&self.vaults, spec);
            out.push(Tool::new(spec.name, spec.description, Arc::new(schema)));
        }
        out
    }

    /// Runs one call and wraps the outcome the way every client expects:
    /// a string as it is, any other value as `JSON.stringify(v, null, 2)`,
    /// an error as `Error: ...` with `isError`.
    pub async fn call(&self, name: &str, args: JsonObject) -> CallToolResult {
        match self.dispatch(name, args).await {
            Ok(Value::String(text)) => CallToolResult::success(vec![ContentBlock::text(text)]),
            Ok(value) => {
                CallToolResult::success(vec![ContentBlock::text(js::json_stringify_pretty(&value))])
            }
            Err(err) => {
                let text = match err.downcast_ref::<InvalidArguments>() {
                    Some(InvalidArguments(problems)) => format!(
                        "MCP error -32602: Input validation error: Invalid arguments for tool {name}: {problems}"
                    ),
                    None => format!("Error: {err:#}"),
                };
                CallToolResult::error(vec![ContentBlock::text(text)])
            }
        }
    }

    async fn dispatch(&self, name: &str, args: JsonObject) -> anyhow::Result<Value> {
        if name == LIST_VAULTS {
            return Ok(self.vaults.describe());
        }
        let &i = self
            .by_name
            .get(name)
            .ok_or_else(|| anyhow::anyhow!("Tool {name} not found"))?;
        let spec = self.tools[i].clone();
        let args = Value::Object(args);
        if let Some(problems) = self.validators[i].check(&args) {
            return Err(InvalidArguments(problems).into());
        }
        let Value::Object(mut args) = args else {
            unreachable!("built as an object")
        };
        // The schema made `vault` one of the names.
        let ctx: Arc<VaultContext> = args
            .remove("vault")
            .and_then(|v| self.vaults.get(v.as_str()?))
            .ok_or_else(|| InvalidArguments("Invalid input at vault".into()))?;
        let timeout = tool_timeout(name);
        match tokio::time::timeout(timeout, spec.call(ctx, args)).await {
            Ok(result) => result,
            Err(_) => Err(anyhow::anyhow!(
                "Tool \"{name}\" timed out after {}ms. This usually means a background operation (embedder, file lock, or index lookup) is stuck.",
                timeout.as_millis()
            )),
        }
    }
}

/// The input schema of `spec` with the required `vault` argument first.
fn input_schema(vaults: &Vaults, spec: &ToolSpec) -> JsonObject {
    let mut schema = spec.input_schema.clone();
    let mut props = JsonObject::new();
    props.insert(
        "vault".into(),
        json!({
            "type": "string",
            "enum": vaults.names(),
            "description": VAULT_ARG_DESCRIPTION,
        }),
    );
    if let Some(Value::Object(rest)) = schema.remove("properties") {
        props.extend(rest);
    }
    schema.insert("properties".into(), Value::Object(props));
    let mut required = vec![Value::from("vault")];
    if let Some(Value::Array(rest)) = schema.remove("required") {
        required.extend(rest);
    }
    schema.insert("required".into(), Value::Array(required));
    schema
}

/// `OBSIDIAN_BRAIN_TOOL_TIMEOUT_MS_<TOOL>`, then
/// `OBSIDIAN_BRAIN_TOOL_TIMEOUT_MS`, then 10 minutes for `reindex` and 30
/// seconds for every other tool.
pub fn tool_timeout(name: &str) -> Duration {
    let parse = |key: String| {
        std::env::var(key)
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .filter(|&ms| ms > 0)
    };
    let per_tool = format!(
        "OBSIDIAN_BRAIN_TOOL_TIMEOUT_MS_{}",
        name.to_uppercase().replace('-', "_")
    );
    let ms = parse(per_tool)
        .or_else(|| parse("OBSIDIAN_BRAIN_TOOL_TIMEOUT_MS".into()))
        .unwrap_or(if name == "reindex" { 600_000 } else { 30_000 });
    Duration::from_millis(ms)
}

impl ServerHandler for Server {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new(SERVER_NAME, env!("CARGO_PKG_VERSION")))
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, McpError> {
        Ok(ListToolsResult {
            tools: self.tool_list(),
            ..Default::default()
        })
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, McpError> {
        let args = request.arguments.unwrap_or_default();
        Ok(CallToolResponse::Complete(
            self.call(&request.name, args).await,
        ))
    }
}
