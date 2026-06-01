use std::borrow::Cow;
use std::fmt::Write;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use rmcp::handler::server::{router::tool::ToolRouter, wrapper::Parameters};
use rmcp::model::{CallToolResult, Content, ServerCapabilities, ServerInfo};
use rmcp::transport::stdio;
use rmcp::{ErrorData, ServiceExt, tool};
use tokio::sync::Mutex;
use zti_ipc_client::Client;
use zti_protocol::format_search_results;
use zti_protocol::request::{DoctorReq, Request, SearchDepReq, SearchMode, SearchReq};
use zti_protocol::response::{CheckStatus, Response, SearchResults};

#[derive(Debug, serde::Deserialize, rmcp::schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct FileTreeParams {
    #[schemars(
        description = "Project name, index number, or root path. Use `projectList` to see available projects."
    )]
    pub project: String,
    #[schemars(
        description = "Optional glob pattern to filter files, e.g. \"**/*.rs\" or \"src/**/*.ts\"."
    )]
    pub path_glob: Option<String>,
}

#[derive(Debug, serde::Deserialize, rmcp::schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SearchQueryParams {
    #[schemars(
        description = "What you're looking for, in natural language. Use descriptive phrases: \"polynomial inversion\" not \"invert\"."
    )]
    pub text: String,
    #[schemars(
        description = "Project name, index number, or root path. Auto-resolved when omitted."
    )]
    pub project: Option<String>,
    #[schemars(description = "Maximum results to return (default: 5).")]
    pub limit: Option<usize>,
    #[schemars(
        description = "Glob pattern to filter files, e.g. \"**/*.rs\" or \"src/**/*.ts\"."
    )]
    pub path_glob: Option<String>,
    #[schemars(
        description = "Language filter, e.g. [\"rust\", \"dart\"]."
    )]
    pub languages: Option<Vec<String>>,
}

#[derive(Debug, serde::Deserialize, rmcp::schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SearchPassageParams {
    #[schemars(
        description = "A code snippet, error message, or descriptive paragraph to find similar implementations."
    )]
    pub text: String,
    #[schemars(
        description = "Project name, index number, or root path. Auto-resolved when omitted."
    )]
    pub project: Option<String>,
    #[schemars(description = "Maximum results to return (default: 5).")]
    pub limit: Option<usize>,
    #[schemars(
        description = "Glob pattern to filter files, e.g. \"**/*.rs\" or \"src/**/*.ts\"."
    )]
    pub path_glob: Option<String>,
    #[schemars(
        description = "Language filter, e.g. [\"rust\", \"dart\"]."
    )]
    pub languages: Option<Vec<String>>,
}

#[derive(Debug, serde::Deserialize, rmcp::schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SearchDepParams {
    #[schemars(description = "Symbol, type, or function name. Bare (\"connect\"), scoped \
        (\"network::connect\"), or fully-qualified (\"myapp::network::connect\"). Use `::` \
        separators in any language.")]
    pub name: String,
    #[schemars(description = "Project name, index, or root path. Auto-resolved when omitted. To learn \
        an external dependency, index its source as a project first, then target it here.")]
    pub project: Option<String>,
    #[schemars(description = "Call-graph depth for callers/callees (default 2).")]
    pub depth: Option<usize>,
}

#[derive(Debug, serde::Deserialize, rmcp::schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DoctorParams {
    #[schemars(
        description = "Project name, index number, or root path. If omitted, runs general diagnostics."
    )]
    pub project: Option<String>,
}

#[derive(Debug, serde::Deserialize, rmcp::schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProjectListParams {}

#[derive(Clone)]
struct ZebraMcpServer {
    #[allow(dead_code)]
    tool_router: ToolRouter<Self>,
    daemon: Arc<Mutex<Option<Client>>>,
    indexed_projects_roots: String,
}

impl ZebraMcpServer {
    fn new(indexed_projects_roots: String) -> Self {
        Self {
            tool_router: Self::tool_router(),
            daemon: Arc::new(Mutex::new(None)),
            indexed_projects_roots,
        }
    }

    async fn ensure_daemon(
        &self,
    ) -> Result<tokio::sync::MutexGuard<'_, Option<Client>>, ErrorData> {
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

    async fn send_search(&self, req: SearchReq) -> Result<SearchResults, ErrorData> {
        let mut guard = self.ensure_daemon().await?;
        let client = guard
            .as_mut()
            .ok_or_else(|| internal_err("daemon not initialized".into()))?;

        match client.request(Request::Search(req)).await {
            Ok(Response::Search(Ok(results))) => Ok(results),
            Ok(Response::Search(Err(e))) => Err(internal_err(e.message)),
            Ok(other) => Err(internal_err(format!("unexpected response: {other:?}"))),
            Err(e) => {
                *guard = None;
                Err(internal_err(format!("IPC lost, retry: {e}")))
            }
        }
    }

    async fn do_search(
        &self,
        text: String,
        project: Option<&str>,
        limit: Option<usize>,
        path_glob: Option<String>,
        languages: Option<Vec<String>>,
        mode: SearchMode,
    ) -> Result<CallToolResult, ErrorData> {
        let project_root = zti_store::resolve_project(project)
            .await
            .map_err(|e| internal_err(format!("{e}")))?;
        let limit = limit.unwrap_or(5);

        let mut req = SearchReq {
            project_root,
            query: text,
            limit,
            offset: None,
            languages,
            path_glob,
            refresh_index: false,
            exhaustive: false,
            mode,
        };

        let results = self.send_search(req.clone()).await?;

        if !results.hits.is_empty() {
            return Ok(ok_text(format_search_results(&results)));
        }

        req.exhaustive = true;
        let retry_results = self.send_search(req).await?;

        Ok(ok_text(format_search_results(&retry_results)))
    }

    async fn send_search_dep(&self, req: SearchDepReq) -> Result<String, ErrorData> {
        let mut guard = self.ensure_daemon().await?;
        let client = guard
            .as_mut()
            .ok_or_else(|| internal_err("daemon not initialized".into()))?;
        match client.request(Request::DslSearchDep(req)).await {
            Ok(Response::DslSearchDep(Ok(body))) => Ok(body.text),
            Ok(Response::DslSearchDep(Err(e))) => Err(internal_err(e.message)),
            Ok(other) => Err(internal_err(format!("unexpected response: {other:?}"))),
            Err(e) => {
                *guard = None;
                Err(internal_err(format!("IPC lost, retry: {e}")))
            }
        }
    }
}

fn match_file(file_path: &str, root: &str, matcher: Option<&globset::GlobMatcher>) -> bool {
    let Some(m) = matcher else { return true };
    let rel = file_path
        .strip_prefix(root)
        .unwrap_or(file_path)
        .trim_start_matches('/');
    m.is_match(rel) || m.is_match(file_path)
}

fn ok_text(text: impl Into<String>) -> CallToolResult {
    CallToolResult::success(vec![Content::text(text.into())])
}

fn internal_err(msg: String) -> ErrorData {
    ErrorData::internal_error(msg, None)
}

fn format_project_table(projects: &[zti_store::ProjectRow]) -> String {
    let mut out = String::with_capacity(projects.len() * 80);
    out.push_str("| # | Project | Root |\n");
    out.push_str("|---|---------|------|\n");
    for (i, p) in projects.iter().enumerate() {
        let name = std::path::Path::new(&p.root_path)
            .file_name()
            .map(|s| s.to_string_lossy())
            .unwrap_or_else(|| Cow::Borrowed(&p.root_path));
        let _ = writeln!(out, "| {} | {} | {} |", i + 1, name, p.root_path);
    }
    out
}

#[rmcp::tool_router]
impl ZebraMcpServer {
    #[tool(
        name = "fileTree",
        description = "List project files and directory structure. Use this instead of `find`, `ls -R`, or `glob` to discover source files — reads from the pre-built project index and returns instantly."
    )]
    async fn file_tree(
        &self,
        Parameters(params): Parameters<FileTreeParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let root_path = zti_store::resolve_project(Some(&params.project))
            .await
            .map_err(|e| internal_err(format!("{e}")))?;
        let root = std::path::Path::new(&root_path);
        let pid = zti_common::ids::project_id(root);
        let db = zti_store::Db::open(&pid)
            .await
            .map_err(|e| internal_err(format!("store open: {e}")))?;
        let files = db
            .files_table()
            .await
            .map_err(|e| internal_err(format!("files_table: {e}")))?
            .list()
            .await
            .map_err(|e| internal_err(format!("list files: {e}")))?;

        let root_str = root.to_string_lossy();

        let matcher = params
            .path_glob
            .as_deref()
            .map(|p| {
                globset::Glob::new(p)
                    .map_err(|e| internal_err(format!("bad glob: {e}")))
                    .map(|g| g.compile_matcher())
            })
            .transpose()?;

        let mut matched = Vec::with_capacity(files.len());
        for f in &files {
            if match_file(&f.file_path, &root_str, matcher.as_ref()) {
                matched.push(f);
            }
        }

        let mut out = String::with_capacity(32 + matched.len() * 80);
        out.push_str("FILES\n");
        for (i, &f) in matched.iter().enumerate() {
            let rel = f
                .file_path
                .strip_prefix(root_str.as_ref())
                .unwrap_or(&f.file_path)
                .trim_start_matches('/');
            let _ = writeln!(out, "#{} [{}] {}", i, f.language, rel);
        }

        if matched.is_empty() {
            out.push_str("  (no files indexed)\n");
        }

        Ok(ok_text(out))
    }

    #[tool(
        name = "searchQuery",
        description = "Search the codebase by intent. Use this FIRST when exploring code, answering questions about the codebase, or finding implementations — before grep, find, or reading files. Describe what you need in plain language (e.g. \"polynomial inversion\", \"error retry logic\"). Returns complete source code with file paths and line ranges — no follow-up file reads needed."
    )]
    async fn search_query(
        &self,
        Parameters(params): Parameters<SearchQueryParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.do_search(
            params.text,
            params.project.as_deref(),
            params.limit,
            params.path_glob,
            params.languages,
            SearchMode::Query,
        )
        .await
    }

    #[tool(
        name = "searchPassage",
        description = "Find similar code by example. Paste a code snippet, error message, or pattern description to locate related implementations. Use this instead of grepping for exact matches when you want semantically similar code."
    )]
    async fn search_passage(
        &self,
        Parameters(params): Parameters<SearchPassageParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.do_search(
            params.text,
            params.project.as_deref(),
            params.limit,
            params.path_glob,
            params.languages,
            SearchMode::Passage,
        )
        .await
    }

    #[tool(
        name = "doctor",
        description = "DEBUG ONLY: Run diagnostics on the embedding engine and index. Use this ONLY when searchQuery or searchPassage return errors — not for empty results."
    )]
    async fn doctor(
        &self,
        Parameters(params): Parameters<DoctorParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let project_root = match &params.project {
            Some(p) => Some(
                zti_store::resolve_project(Some(p))
                    .await
                    .map_err(|e| internal_err(format!("{e}")))?,
            ),
            None => None,
        };
        let req = DoctorReq { project_root };

        let mut guard = self.ensure_daemon().await?;
        let client = guard
            .as_mut()
            .ok_or_else(|| internal_err("daemon not initialized".into()))?;

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

    #[tool(
        name = "searchDep",
        description = "Look up a symbol by exact name and get its definition with call graph in one \
            call: kind, location, doc summary, callers and callees (to `depth`), and full source \
            body — token-budgeted, no file reads. Use when you already know the name (type, \
            function, method, or a dependency's interface) and want its implementation and how it \
            connects. For conceptual/fuzzy search use searchQuery; for find-by-example use \
            searchPassage. Qualify with `::` in any language (e.g. \"runtime::Runtime\"). An \
            ambiguous bare name returns a candidate list to retry with a qualified path. Only \
            indexed projects are searchable."
    )]
    async fn search_dep(
        &self,
        Parameters(params): Parameters<SearchDepParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let project_root = zti_store::resolve_project(params.project.as_deref())
            .await
            .map_err(|e| internal_err(format!("{e}")))?;
        let req = SearchDepReq {
            project_root,
            name: params.name,
            depth: params.depth,
            max_tokens: None,
        };
        let out = self.send_search_dep(req).await?;
        Ok(ok_text(out))
    }

    #[tool(
        name = "projectList",
        description = "Lists all indexed projects with root paths and stats. Call this when you need the `project` parameter for other tools and are unsure which project to target."
    )]
    async fn project_list(
        &self,
        Parameters(_): Parameters<ProjectListParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let projects = zti_store::list_projects()
            .await
            .map_err(|e| internal_err(format!("list_projects: {e}")))?;

        if projects.is_empty() {
            return Ok(ok_text("No indexed projects found."));
        }

        Ok(ok_text(format_project_table(&projects)))
    }
}

#[rmcp::tool_handler]
impl rmcp::ServerHandler for ZebraMcpServer {
    fn get_info(&self) -> ServerInfo {
        let mut info = ServerInfo::default();
        let mut instructions = String::with_capacity(1024 + self.indexed_projects_roots.len());
        instructions.push_str(
            "# zebraindex — Semantic Code Search\n\
             \n\
             ## When to use these tools\n\
             \n\
             Use `searchQuery` as your **first step** when exploring code, answering \
             questions, or locating implementations. It replaces grep, find, and \
             manual file browsing — it understands what you mean, not just what you \
             type, and returns complete source code in a single call.\n\
             \n\
             ## Workflow\n\
             \n\
             1. **Start with `searchQuery`** — describe what you're looking for \
             in natural language. Results include the full source code with \
             file paths and line ranges. No second read step needed.\n\
             \n\
             2. **Use `searchPassage`** when you have a code snippet or error \
             message and want to find similar patterns across the project.\n\
             \n\
             3. **Use `fileTree`** to discover project structure — prefer it \
             over `find` or `ls`.\n\
              \n\
              ## Tips\n\
              \n\
              * Use descriptive phrases, not single keywords. \
              \"user session validation\" finds more than \"auth\".\n\
              * The `project` parameter accepts a project name, index number, \
              or root path. It auto-resolves when omitted.\n\
              * Results contain complete source code — use it directly without \
              re-reading files.\n\
              * If the fast index misses results, exhaustive search runs \
              automatically.\n\
              \n\
              ## Learning a Dependency\n\
              \n\
              To inspect an external library, index its source as a project, \
              then use `searchDep` on any symbol.",
        );
        instructions.push_str(&self.indexed_projects_roots);
        info.instructions = Some(instructions);
        info.capabilities = ServerCapabilities::builder().enable_tools().build();
        info
    }
}

pub fn run_mcp() -> Result<()> {
    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(async {
        let indexed_projects_roots = match zti_store::list_projects().await {
            Ok(projects) if projects.len() > 1 => {
                let mut s = String::with_capacity(32 + projects.len() * 80);
                s.push_str("\n\n## Indexed Projects\n\n");
                s.push_str(&format_project_table(&projects));
                s
            }
            _ => String::new(),
        };

        let server = ZebraMcpServer::new(indexed_projects_roots);
        let service = server.serve(stdio()).await?;
        service.waiting().await?;

        Ok(())
    })
}
