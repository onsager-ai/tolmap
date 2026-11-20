//! Stdio adapter for the read-only CLI query commands.

use std::path::{Path, PathBuf};

use anyhow::{ensure, Context, Result};
use clap::{Arg, ArgAction, CommandFactory, Parser};
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, Content, Implementation, JsonObject,
    ListToolsResult, PaginatedRequestParams, ServerCapabilities, ServerConfig, Tool,
    ToolAnnotations,
};
use rmcp::service::RequestContext;
use rmcp::{ErrorData, RoleServer, ServerHandler, ServiceExt};
use serde_json::{json, Value};

use crate::{run_query, Cli, Command};

const TOOLS: [&str; 8] = [
    "status", "repos", "overview", "where", "deps", "route", "debt", "check",
];

struct Queries {
    data: Option<PathBuf>,
}

#[tokio::main(flavor = "current_thread")]
pub fn serve(data: Option<PathBuf>) -> Result<()> {
    Queries { data }
        .serve(rmcp::transport::stdio_pipes())
        .await?
        .waiting()
        .await?;
    Ok(())
}

impl ServerHandler for Queries {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("tolmap", env!("CARGO_PKG_VERSION")))
    }

    async fn list_tools(
        &self,
        _: Option<PaginatedRequestParams>,
        _: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        let cli = Cli::command();
        let tools = TOOLS
            .iter()
            .map(|&name| {
                let command = cli
                    .find_subcommand(name)
                    .expect("MCP tool has a CLI command");
                let description = match name {
                    "check" => {
                        "Report the structural impact of a change with no pass/fail thresholds."
                            .into()
                    }
                    "debt" => "Report structural debt in a stored map.".into(),
                    _ => command
                        .get_about()
                        .expect("query has help text")
                        .to_string(),
                };
                Tool::new(name, description, schema(command))
                    .with_annotations(ToolAnnotations::new().read_only(true).open_world(false))
            })
            .collect();
        Ok(ListToolsResult::with_all_items(tools))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        if !TOOLS.contains(&request.name.as_ref()) {
            return Err(ErrorData::invalid_params(
                format!("unknown tool: {}", request.name),
                None,
            ));
        }
        let data = self.data.clone();
        let result = tokio::task::spawn_blocking(move || execute(request, data.as_deref())).await;
        let result = match result {
            Ok(Ok(text)) => CallToolResult::success(vec![Content::text(text)]),
            Ok(Err(error)) => CallToolResult::error(vec![Content::text(error)]),
            Err(error) => return Err(ErrorData::internal_error(error.to_string(), None)),
        };
        Ok(result.into())
    }
}

// Keep names, accepted values, and CLI validation in clap. The server fixes the
// output format and check mode, and the data root belongs to the server process.
fn inputs(command: &clap::Command) -> impl Iterator<Item = &Arg> {
    command.get_arguments().filter(|arg| {
        !matches!(
            arg.get_id().as_str(),
            "data" | "repo_flag" | "format" | "report_only" | "max_districts" | "max_dq"
        )
    })
}

fn name(arg: &Arg) -> &str {
    arg.get_long().unwrap_or_else(|| arg.get_id().as_str())
}

fn is_integer(arg: &Arg) -> bool {
    matches!(arg.get_id().as_str(), "limit" | "top")
}

fn schema(command: &clap::Command) -> JsonObject {
    let mut properties = JsonObject::new();
    let mut required = Vec::new();
    for arg in inputs(command) {
        let field = name(arg);
        let values = arg.get_possible_values();
        let mut property = match arg.get_action() {
            ArgAction::SetTrue => json!({"type": "boolean", "default": false}),
            _ if is_integer(arg) => json!({"type": "integer", "minimum": 0}),
            _ if values.is_empty() => json!({"type": "string"}),
            _ => {
                json!({"type": "string", "enum": values.iter().map(|value| value.get_name()).collect::<Vec<_>>()})
            }
        };
        if matches!(arg.get_action(), ArgAction::Append) {
            property = json!({"type": "array", "items": property});
        }
        if let Some(default) = arg
            .get_default_values()
            .first()
            .and_then(|value| value.to_str())
        {
            property["default"] = json!(default);
        }
        if let Some(help) = arg.get_help() {
            property["description"] = json!(help.to_string());
        }
        if arg.is_required_set() {
            required.push(field);
        }
        properties.insert(field.into(), property);
    }
    JsonObject::from_iter([
        ("type".into(), json!("object")),
        ("properties".into(), Value::Object(properties)),
        ("required".into(), json!(required)),
        ("additionalProperties".into(), json!(false)),
    ])
}

fn parse(tool: &str, mut arguments: JsonObject) -> Result<Command> {
    let cli = Cli::command();
    let command = cli
        .find_subcommand(tool)
        .expect("MCP tool has a CLI command");
    let mut argv = vec![
        "tolmap".to_owned(),
        tool.to_owned(),
        "--format=json".to_owned(),
    ];
    if tool == "check" {
        argv.push("--report-only".into());
    }
    let mut positionals = Vec::new();
    for arg in inputs(command) {
        let field = name(arg);
        let Some(value) = arguments.remove(field) else {
            ensure!(!arg.is_required_set(), "missing argument: {field}");
            continue;
        };
        match arg.get_action() {
            ArgAction::SetTrue => {
                if value
                    .as_bool()
                    .with_context(|| format!("{field} must be a boolean"))?
                {
                    argv.push(format!("--{field}"));
                }
            }
            ArgAction::Append => {
                let values = value
                    .as_array()
                    .with_context(|| format!("{field} must be an array"))?;
                for value in values {
                    let value = value
                        .as_str()
                        .with_context(|| format!("{field} must contain strings"))?;
                    argv.push(format!("--{field}={value}"));
                }
            }
            _ if is_integer(arg) => {
                let value = value
                    .as_u64()
                    .with_context(|| format!("{field} must be a non-negative integer"))?;
                argv.push(format!("--{field}={value}"));
            }
            _ => {
                let value = value
                    .as_str()
                    .with_context(|| format!("{field} must be a string"))?;
                if arg.get_long().is_some() {
                    argv.push(format!("--{field}={value}"));
                } else {
                    positionals.push(value.to_owned());
                }
            }
        }
    }
    ensure!(
        arguments.is_empty(),
        "unknown arguments: {}",
        arguments.keys().cloned().collect::<Vec<_>>().join(", ")
    );
    // Values must stay literal, even when they begin with '-' or name a subcommand.
    if !positionals.is_empty() {
        argv.push("--".into());
        argv.extend(positionals);
    }
    Ok(Cli::try_parse_from(argv)?.command)
}

fn execute(request: CallToolRequestParams, data: Option<&Path>) -> Result<String, String> {
    let command = parse(&request.name, request.arguments.unwrap_or_default())
        .map_err(|error| error.to_string())?;
    let mut output = Vec::new();
    let mut diagnostics = Vec::new();
    match run_query(command, data, &mut output, &mut diagnostics) {
        Ok(0) => Ok(String::from_utf8(output).expect("query output is UTF-8")),
        Ok(_) => Err(String::from_utf8(diagnostics).expect("query diagnostics are UTF-8")),
        Err(error) => Err(format!("Error: {error:?}\n")),
    }
}
