use std::borrow::Cow;
use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::fmt::Write;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use clap::Parser;
use rmcp::handler::server::{router::tool::ToolRouter, wrapper::Parameters};
use rmcp::model::{CallToolResult, Content, ServerCapabilities, ServerInfo};
use rmcp::transport::stdio;
use rmcp::{ErrorData, ServiceExt, tool};
use tokio::sync::{Mutex, RwLock};
use tracing_subscriber::EnvFilter;
use zti_common::format::format_elapsed;
use zti_dsl::{
    AsciiTreeRenderer, ProjectIndex, build_index, render::dsl::DslRenderer,
    render::dsl::render_files_only,
};
use zti_ipc_client::Client;
use zti_protocol::format_search_results;
use zti_protocol::request::{DoctorReq, Request, SearchReq, SearchMode};
use zti_protocol::response::{CheckStatus, Response};
use zti_tree_sitter::{parse_kinds, parse_language};

#[derive(Parser)]
#[command(name = "zebra-mcp", about = "Zebra MCP server (DSL + daemon IPC, stdio)")]
struct Cli;

#[derive(Debug, serde::Deserialize, rmcp::schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct FileTreeParams {
    pub project_root: String,
    pub path_glob: Option<String>,
}

#[derive(Debug, serde::Deserialize, rmcp::schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProjectMapParams {
    pub project_root: String,
    pub language: Option<String>,
    pub path_glob: Option<String>,
    pub kinds: Option<Vec<String>>,
    pub max_tokens: Option<usize>,
}

#[derive(Debug, serde::Deserialize, rmcp::schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DepTreeParams {
    pub project_root: String,
    pub symbol_id: u32,
    pub direction: String,
    pub depth: Option<usize>,
}

#[derive(Debug, serde::Deserialize, rmcp::schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ReadCodeParams {
    pub project_root: String,
    pub symbol_ids: Vec<u32>,
}

#[derive(Debug, serde::Deserialize, rmcp::schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SearchParams {
    pub project_root: String,
    pub query: String,
    pub limit: Option<usize>,
    #[schemars(description = "When true, brute-force scan ALL embeddings instead of the fast approximate index. More accurate but significantly slower. Use only when approximate search misses relevant results.")]
    pub exhaustive: Option<bool>,
    #[schemars(description = "How the embedding model encodes the query: \"query\" (default, best for short keyword searches like 'find the auth handler') or \"passage\" (for longer descriptive input).")]
    pub mode: Option<String>,
}

#[derive(Debug, serde::Deserialize, rmcp::schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DoctorParams {
    pub project_root: Option<String>,
}

#[derive(Debug, serde::Deserialize, rmcp::schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProjectListParams {}

#[derive(Clone)]
struct ZebraMcpServer {
    #[allow(dead_code)]
    tool_router: ToolRouter<Self>,
    indexes: Arc<RwLock<HashMap<String, Arc<ProjectIndex>>>>,
    daemon: Arc<Mutex<Option<Client>>>,
}

impl ZebraMcpServer {
    fn new() -> Self {
        Self {
            tool_router: Self::tool_router(),
            indexes: Arc::new(RwLock::new(HashMap::with_capacity(4))),
            daemon: Arc::new(Mutex::new(None)),
        }
    }

    async fn get_index(&self, project_root: &str) -> Result<Arc<ProjectIndex>, ErrorData> {
        let root = std::path::Path::new(project_root)
            .canonicalize()
            .map_err(|e| internal_err(format!("invalid project_root: {e}")))?;
        let root_key = root.to_string_lossy().to_string();

        let mut guard = self.indexes.write().await;
        match guard.entry(root_key) {
            Entry::Occupied(e) => Ok(Arc::clone(e.get())),
            Entry::Vacant(e) => {
                let key = e.key().clone();
                let idx = Arc::new(
                    tokio::task::spawn_blocking(move || build_index(&key))
                        .await
                        .map_err(|e| internal_err(format!("indexing task failed: {e}")))?
                        .map_err(|e| internal_err(format!("indexing failed: {e}")))?,
                );
                e.insert(Arc::clone(&idx));
                Ok(idx)
            }
        }
    }

    async fn ensure_daemon(&self) -> Result<tokio::sync::MutexGuard<'_, Option<Client>>, ErrorData> {
        let mut guard = self.daemon.lock().await;
        if guard.is_none() {
            let mut client = Client::connect(Duration::from_secs(10), None, None, None, None)
                .await
                .map_err(|e| internal_err(format!("daemon connect: {e}")))?;
            client
                .handshake()
                .await
                .map_err(|e| internal_err(format!("handshake: {e}")))?;
            *guard = Some(client);
        }
        Ok(guard)
    }
}

fn ok_text(text: String) -> CallToolResult {
    CallToolResult::success(vec![Content::text(text)])
}

fn internal_err(msg: String) -> ErrorData {
    ErrorData::internal_error(msg, None)
}

#[rmcp::tool_router]
impl ZebraMcpServer {
    #[tool(name = "fileTree", description = "Get the project structure. Use this to understand the directory layout or find specific files. Returns paths and their numeric #IDs.")]
    async fn file_tree(
        &self,
        Parameters(params): Parameters<FileTreeParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let index = self.get_index(&params.project_root).await?;

        let file_indices: Vec<u16> = if let Some(glob) = &params.path_glob {
            zti_dsl::glob_match_files(&index.files, &index.root, glob)
                .map_err(|e| internal_err(format!("bad glob: {e}")))?
        } else {
            (0..index.files.len() as u16).collect()
        };

        let mut out = render_files_only(&index, &file_indices);
        out.push_str("\n\n[SYSTEM HINT: To see symbols inside a file, use `projectMap` with the same project_root. To search for specific logic, use `search`.]");
        Ok(ok_text(out))
    }

    #[tool(name = "projectMap", description = "Get a high-level AST graph of classes, structs, and functions. Use this when you need an overview of what's inside a file or language. Returns symbol names and their critical #IDs.")]
    async fn project_map(
        &self,
        Parameters(params): Parameters<ProjectMapParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let index = self.get_index(&params.project_root).await?;
        let max_tokens = params.max_tokens.unwrap_or(8000);

        let lang = params.language.as_deref().and_then(parse_language);
        let file_filter = zti_dsl::filter_files(
            &index.files,
            &index.root,
            params.path_glob.as_deref(),
            lang,
        )
        .map_err(internal_err)?;

        let kind_filter = params.kinds.as_ref().map(|k| parse_kinds(k));

        let renderer = DslRenderer::new(&index, max_tokens);
        let mut out = renderer.render(Some(&file_filter), kind_filter.as_deref());
        out.push_str("\n\n[SYSTEM HINT: To read source code, use `readCode` with the #ID wrapped in an array (e.g. [ID]). To trace dependencies, use `depTree`.]");
        Ok(ok_text(out))
    }

    #[tool(name = "depTree", description = "CRITICAL FOR REFACTORING: Trace dependencies. Pass a known symbol #ID and direction ('callers' or 'callees') to see exactly where a function is used or what it depends on.")]
    async fn dep_tree(
        &self,
        Parameters(params): Parameters<DepTreeParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let index = self.get_index(&params.project_root).await?;
        let depth = params.depth.unwrap_or(2);

        let renderer = AsciiTreeRenderer::new(&index);
        let text = match params.direction.as_str() {
            "callers" => renderer.render_callers(params.symbol_id, depth),
            "callees" => renderer.render_callees(params.symbol_id, depth, false),
            other => {
                return Err(internal_err(format!(
                    "direction must be 'callers' or 'callees', got '{other}'"
                )));
            }
        };

        Ok(ok_text(text))
    }

    #[tool(name = "readCode", description = "READ CODE: Fetches exact source code. Pass an array of ONE OR MORE #IDs (e.g. [204] or [12, 45, 99]). RULE: You MUST obtain the #IDs from `search` or `projectMap` first. NEVER guess IDs.")]
    async fn read_code(
        &self,
        Parameters(params): Parameters<ReadCodeParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let index = self.get_index(&params.project_root).await?;
        let entries = zti_dsl::resolve_symbol_bodies(&index, &params.symbol_ids);

        let mut out = String::with_capacity(entries.len() * 256);
        for entry in &entries {
            let _ = writeln!(out, "{}\n---", entry);
        }

        Ok(ok_text(out))
    }

    #[tool(name = "search", description = "START HERE: Semantic search. Use this first when looking for concepts, logic, or features by natural language (e.g., 'how does auth work'). Returns ranked snippets and symbol #IDs.")]
    async fn search(
        &self,
        Parameters(params): Parameters<SearchParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let mode = params
            .mode
            .as_deref()
            .map(|m| m.parse::<SearchMode>().unwrap_or_default())
            .unwrap_or_default();

        let req = SearchReq {
            project_root: params.project_root,
            query: params.query,
            limit: params.limit.unwrap_or(5),
            offset: None,
            languages: None,
            path_glob: None,
            refresh_index: false,
            exhaustive: params.exhaustive.unwrap_or(false),
            mode,
        };

        let mut guard = self.ensure_daemon().await?;
        let client = guard.as_mut().unwrap();

        match client.request(Request::Search(req)).await {
            Ok(Response::Search(Ok(results))) => {
                let mut out = format_search_results(&results);
                out.push_str("\n\n[SYSTEM HINT: To read the full code for any result, use `readCode` with the #ID wrapped in an array (e.g. [ID]). To trace where it's used, use `depTree` with direction 'callers'.]");
                Ok(ok_text(out))
            }
            Ok(Response::Search(Err(e))) => Err(internal_err(e.message)),
            Ok(other) => Err(internal_err(format!("unexpected: {other:?}"))),
            Err(e) => {
                *guard = None;
                Err(internal_err(format!("IPC lost, retry: {e}")))
            }
        }
    }

    #[tool(name = "doctor", description = "DEBUG ONLY: Run diagnostics on the embedding engine and database. Use this ONLY if the `search` tool returns system errors or fails to connect.")]
    async fn doctor(
        &self,
        Parameters(params): Parameters<DoctorParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let req = DoctorReq {
            project_root: params.project_root,
        };

        let mut guard = self.ensure_daemon().await?;
        let client = guard.as_mut().unwrap();

        match client.request(Request::Doctor(req)).await {
            Ok(Response::Doctor(Ok(report))) => {
                let mut out = String::with_capacity(256 + report.checks.len() * 64);
                let _ = writeln!(out, "Device: {}", report.device);
                for check in &report.checks {
                    let marker = match check.status {
                        CheckStatus::Ok => "OK",
                        CheckStatus::Warn => "WARN",
                        CheckStatus::Err => "ERR",
                    };
                    let _ = writeln!(out, "[{}] {}: {}", marker, check.name, check.message);
                }
                Ok(ok_text(out))
            }
            Ok(Response::Doctor(Err(e))) => Err(internal_err(e.message)),
            Ok(other) => Err(internal_err(format!("unexpected: {other:?}"))),
            Err(e) => {
                *guard = None;
                Err(internal_err(format!("IPC lost, retry: {e}")))
            }
        }
    }

    #[tool(name = "projectList", description = "ENVIRONMENT CHECK: List all available indexed projects. Use this if you don't know the `project_root` path yet.")]
    async fn project_list(
        &self,
        Parameters(_): Parameters<ProjectListParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let projects = zti_store::list_projects()
            .await
            .map_err(|e| internal_err(format!("list_projects: {e}")))?;

        if projects.is_empty() {
            return Ok(ok_text(String::from("No indexed projects found.")));
        }

        let mut out = String::with_capacity(projects.len() * 128);
        out.push_str("| Project | Model | Chunks | Files | Last Indexed |\n");
        out.push_str("|---------|-------|--------|-------|-------------|\n");
        for p in &projects {
            let name = std::path::Path::new(&p.root_path)
                .file_name()
                .map(|s| s.to_string_lossy())
                .unwrap_or_else(|| Cow::Borrowed(&p.root_path));
            let ago = format_elapsed(p.last_indexed_ns);
            let _ = writeln!(
                out,
                "| {} | {} | {} | {} | {} |",
                name, p.model_id, p.total_chunks, p.total_files, ago
            );
        }

        out.push_str("\n\n[SYSTEM HINT: To explore a project, use `search` or `fileTree` with the project's root path.]");
        Ok(ok_text(out))
    }
}

#[rmcp::tool_handler]
impl rmcp::ServerHandler for ZebraMcpServer {
    fn get_info(&self) -> ServerInfo {
        let mut info = ServerInfo::default();
        info.instructions = Some(
            "CRITICAL: You are connected to the Zebra Tree Indexer. \
             DO NOT GUESS OR HALLUCINATE code, file paths, or function names. \
             You must extract exact information using these tools. \n\n\
             WORKFLOW:\n\
             1. DISCOVER: Always start with `search` (for natural language) \
             or `projectMap` (for architecture overview). \n\
             2. IDENTIFY: Pay close attention to the numeric `#ID` returned \
             by these tools.\n\
             3. DEEP DIVE: Use `readCode` passing the exact `#ID` wrapped in an array \
             (e.g., [123] or [12, 45]) to read the actual source code.\n\
             4. TRACE: Use `depTree` passing the `#ID` to find callers or callees.\n\n\
             Rule: Never use `readCode` or `depTree` without first finding \
             the correct `#ID`."
                .into(),
        );
        info.capabilities = ServerCapabilities::builder().enable_tools().build();
        info
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("warn")),
        )
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .init();

    let _cli = Cli::parse();
    let server = ZebraMcpServer::new();
    let service = server.serve(stdio()).await?;
    service.waiting().await?;

    Ok(())
}
