//! Workspace-scoped language-server queries and versioned rename previews.
//! A leased client belongs to the
//! query future: cancellation drops the connection, tasks and process tree.
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::Mutex as AsyncMutex;

use super::{ApprovalMode, ToolCall, ToolDefinition, ToolError, ToolRegistry};
use client::Client;
use config::{Language, snapshot};

mod client;
mod config;
mod output;
mod rename;
#[cfg(test)]
mod tests;
mod transport;

const MAX_RESULTS: usize = 200;
const DEFAULT_RESULTS: usize = 60;
const MAX_OPEN_DOCUMENTS: usize = 32;
const DEFAULT_TIMEOUT_SECONDS: u64 = 20;
const MAX_TIMEOUT_SECONDS: u64 = 60;
const MAX_QUERY_BYTES: usize = 256;

type ClientSlot = Arc<AsyncMutex<Option<Client>>>;
type ClientKey = (PathBuf, Language);

#[derive(Default)]
pub(super) struct LspManager {
    clients: Mutex<BTreeMap<ClientKey, ClientSlot>>,
}

impl LspManager {
    fn slot(&self, root: &Path, language: Language) -> ClientSlot {
        self.clients
            .lock()
            .expect("lsp manager")
            .entry((root.to_path_buf(), language))
            .or_default()
            .clone()
    }

    fn status(&self, root: &Path, language: Language) -> Value {
        let slot = self
            .clients
            .lock()
            .expect("lsp manager")
            .get(&(root.to_path_buf(), language))
            .cloned();
        let Some(slot) = slot else {
            return json!({"state":"not_started"});
        };
        match slot.try_lock() {
            Ok(client) => client
                .as_ref()
                .map_or(json!({"state":"not_started"}), |client| {
                    json!({
                        "state":if client.rpc.failure().is_some() {"failed"} else {"running"},
                        "failure":client.rpc.failure(),
                        "capabilities":client.capability_summary(),
                        "open_documents":client.documents.len(),
                        "stderr_retained_bytes":client.rpc.stderr_bytes(),
                        "profile_fingerprint":client.profile,
                    })
                }),
            Err(_) => json!({"state":"busy"}),
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum Operation {
    Status,
    Diagnostics,
    Definition,
    References,
    Symbols,
    RenamePreview,
}

impl Operation {
    fn name(self) -> &'static str {
        match self {
            Self::Status => "status",
            Self::Diagnostics => "diagnostics",
            Self::Definition => "definition",
            Self::References => "references",
            Self::Symbols => "symbols",
            Self::RenamePreview => "rename_preview",
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Arguments {
    operation: Operation,
    path: Option<String>,
    line: Option<u32>,
    character: Option<u32>,
    query: Option<String>,
    new_name: Option<String>,
    limit: Option<usize>,
    timeout_seconds: Option<u64>,
}

pub(super) fn definition() -> ToolDefinition {
    ToolDefinition {
        name: "lsp".into(),
        description: "Query locally installed Rust or TypeScript language servers: status, diagnostics, definition, references, symbols, rename_preview. Positions are 0-based UTF-16. Rename uses two requests to discover and synchronize targets, then returns a one-shot file_changes preview; apply it separately with file_changes.apply. Rust requires versioned documentChanges. Native TypeScript changes are bound to the exact URI and text snapshot synchronized before the final request; missing or stale versions in documentChanges are still refused. Resource operations, annotations and unverified edits are refused as a whole. Limits: 32 files and 4 MiB combined old/new bytes. Fixed trusted toolchains only; no downloads. Runtime command approval and the effective OS sandbox apply on every query. Missing or unversioned diagnostics never prove a clean snapshot.".into(),
        parameters: json!({
            "type":"object", "additionalProperties":false,
            "properties": {
                "operation":{"type":"string","enum":["status","diagnostics","definition","references","symbols","rename_preview"]},
                "path":{"type":"string","description":"Workspace-relative .rs, .ts or .tsx source file. Required except for status."},
                "line":{"type":"integer","minimum":0,"description":"0-based line; required for definition/references/rename_preview."},
                "character":{"type":"integer","minimum":0,"description":"0-based UTF-16 code-unit offset; required for definition/references/rename_preview."},
                "new_name":{"type":"string","minLength":1,"maxLength":rename::MAX_NEW_NAME_BYTES,"description":"rename_preview only: replacement symbol name, without NUL."},
                "query":{"type":"string","maxLength":MAX_QUERY_BYTES,"description":"symbols only: workspace symbol query. Omit for symbols in path."},
                "limit":{"type":"integer","minimum":1,"maximum":MAX_RESULTS},
                "timeout_seconds":{"type":"integer","minimum":1,"maximum":MAX_TIMEOUT_SECONDS}
            }, "required":["operation"]
        }),
    }
}

impl ToolRegistry {
    pub(super) async fn lsp(&self, call: &ToolCall) -> Result<String, ToolError> {
        let args: Arguments = super::parse(call)?;
        let timeout = args.timeout_seconds.unwrap_or(DEFAULT_TIMEOUT_SECONDS);
        if timeout == 0 || timeout > MAX_TIMEOUT_SECONDS {
            return Err(lsp_error("timeout_seconds must be between 1 and 60"));
        }
        let limit = args.limit.unwrap_or(DEFAULT_RESULTS);
        if limit == 0 || limit > MAX_RESULTS {
            return Err(lsp_error("limit must be between 1 and 200"));
        }
        if args
            .query
            .as_ref()
            .is_some_and(|q| q.len() > MAX_QUERY_BYTES)
        {
            return Err(lsp_error("symbol query exceeds 256 bytes"));
        }
        if args.query.is_some() && args.operation != Operation::Symbols {
            return Err(lsp_error("query is only accepted by the symbols operation"));
        }
        rename::validate_arguments(&args).map_err(lsp_error)?;
        if args.operation == Operation::Status {
            return self.lsp_status(args.path.as_deref()).await;
        }
        // Explicitly precede gate_command's one-time/preapproved worker paths.
        if self.approval_mode() == ApprovalMode::ReadOnly || self.delegation_read_only {
            return Err(ToolError::ReadOnlyPolicy("language-server process".into()));
        }
        let requested = args
            .path
            .as_deref()
            .ok_or_else(|| lsp_error("path is required for LSP queries"))?;
        let path = self.resolve_existing(requested)?;
        if path
            .strip_prefix(&self.workspace)
            .is_ok_and(|p| p.components().any(|c| c.as_os_str() == ".git"))
        {
            return Err(lsp_error("Git metadata cannot be queried as LSP source"));
        }
        let language = Language::from_path(&path).map_err(lsp_error)?;
        let duration = Duration::from_secs(timeout);
        // Configuration inspection also has a deadline and runs without starting
        // any process. The same inspection runs after the result is normalized.
        let configuration = tokio::time::timeout(
            duration,
            config::configuration_id(language, &self.workspace),
        )
        .await
        .map_err(|_| lsp_error("LSP configuration inspection timed out"))?
        .map_err(lsp_error)?;
        let launch = config::resolve_launch(language, &self.workspace).map_err(lsp_error)?;
        // Gate the actual fixed executable command, not a convenient alias that
        // could borrow an unrelated narrow worker's command permission.
        self.gate_lsp_launch(language, &launch).await?;
        let fence = execution_fence(self).map_err(lsp_error)?;
        let sandbox = self.effective_sandbox();
        let profile = snapshot(
            serde_json::to_vec(&json!({
                "fence":fence,
                "configuration":configuration, "command":launch.command,
                "options":launch.options, "trusted_path":launch.path,
            }))
            .map_err(|e| lsp_error(e.to_string()))?
            .as_slice(),
        );
        let slot = self.lsp.slot(&self.workspace, language);
        let mut cached = tokio::time::timeout(duration, slot.lock())
            .await
            .map_err(|_| lsp_error("another LSP query is busy; retry after it completes"))?;
        let mut client = cached.take();
        if client
            .as_ref()
            .is_some_and(|c| c.profile != profile || c.rpc.failure().is_some())
        {
            client = None;
        }
        let result = tokio::time::timeout(duration, async {
            // Approval or a preceding query may have taken time. Do not start a
            // server using configuration that changed during those waits.
            if config::configuration_id(language, &self.workspace).await? != configuration {
                return Err("LSP configuration changed while waiting; retry the query".into());
            }
            if execution_fence(self)? != fence {
                return Err(
                    "LSP execution policy changed while waiting; retry under the current policy"
                        .into(),
                );
            }
            if client.is_none() {
                client = Some(
                    Client::start(&self.workspace, language, &sandbox, profile, launch).await?,
                );
            }
            let active = client.as_mut().expect("initialized LSP client");
            let context = active.refresh_documents(&self.workspace, &path).await?;
            if matches!(
                args.operation,
                Operation::Definition | Operation::References | Operation::RenamePreview
            ) {
                validate_position(&context.text, args.line, args.character)?;
            }
            // Rename never passes through the query normalizer: its source
            // cache may read result files after the response, which is not an
            // admissible edit baseline.
            let (rendered, replacements) = if args.operation == Operation::RenamePreview {
                (None, Some(rename::replacements(active, &self.workspace, &args, &context).await?))
            } else {
                let payload = active.query(&args, &context.document).await?;
                (Some(output::render(&self.workspace, &context, args.operation, limit, payload).await?), None)
            };
            client::verify_snapshots(&self.workspace, &context.checked_documents).await?;
            if let Some(rendered) = &rendered {
                client::verify_snapshots(&self.workspace, &rendered.checked_documents).await?;
            }
            if config::configuration_id(language, &self.workspace).await? != configuration {
                return Err(
                    "LSP configuration changed during the query; stale result discarded".into(),
                );
            }
            if execution_fence(self)? != fence {
                return Err(
                    "LSP execution policy changed during the query; connection retired".into(),
                );
            }
            if let Some(replacements) = replacements {
                // Same preview authorization/write-set/lock checks as the
                // explicit tool, without nested execute or duplicate hooks.
                let preview = self.preview_versioned_replacements(replacements)
                    .await.map_err(|error| error.to_string())?;
                if config::configuration_id(language, &self.workspace).await? != configuration
                    || execution_fence(self)? != fence
                {
                    return Err("LSP policy/configuration changed while preparing preview; result discarded".into());
                }
                // The lease revokes the cached ID on error, timeout or task
                // cancellation. No await remains after publishing it.
                Ok::<String, String>(preview.publish())
            } else {
                Ok(rendered.expect("query output").text)
            }
        })
        .await;
        match result {
            Ok(Ok(result)) => {
                *cached = client;
                Ok(result)
            }
            Ok(Err(error)) => Err(lsp_error(error)),
            Err(_) => Err(lsp_error(
                "LSP operation deadline exceeded; connection destroyed and next query will restart it",
            )),
        }
    }

    async fn gate_lsp_launch(
        &self,
        language: Language,
        launch: &config::Launch,
    ) -> Result<(), ToolError> {
        if self.approval_mode() == ApprovalMode::ReadOnly || self.delegation_read_only {
            return Err(ToolError::ReadOnlyPolicy("language-server process".into()));
        }
        self.gate_command(&launch.command, &format!(
            "Start/use {} for workspace LSP queries. Automatic build scripts, proc macros, TypeScript plugins and type acquisition are disabled. The external process is not inherently read-only.", language.command()
        )).await
    }

    async fn lsp_status(&self, requested: Option<&str>) -> Result<String, ToolError> {
        let languages = if let Some(path) = requested {
            vec![Language::from_path(&self.resolve_existing(path)?).map_err(lsp_error)?]
        } else {
            vec![Language::Rust, Language::TypeScript]
        };
        let mut servers = Vec::new();
        for language in languages {
            let configuration = tokio::time::timeout(
                Duration::from_secs(DEFAULT_TIMEOUT_SECONDS),
                config::configuration_id(language, &self.workspace),
            )
            .await;
            let config_status = match configuration {
                Ok(Ok(id)) => json!({"valid":true,"fingerprint":id}),
                Ok(Err(error)) => json!({"valid":false,"reason":error}),
                Err(_) => json!({"valid":false,"reason":"configuration inspection timed out"}),
            };
            let dependency = match config::resolve_launch(language, &self.workspace) {
                Ok(_) => {
                    json!({"available":true,"selection":"trusted installation outside workspace"})
                }
                Err(error) => json!({"available":false,"reason":error}),
            };
            servers.push(json!({
                "language":language.name(), "command":language.command(), "dependency":dependency,
                "installation_hint":language.installation_hint(),
                "initialization_options":language.initialization_options(),
                "configuration":language.configuration(), "configuration_check":config_status,
                "runtime":self.lsp.status(&self.workspace, language),
            }));
        }
        Ok(json!({
            "tool":"lsp", "operation":"status", "servers":servers,
            "position_encoding":"0-based UTF-16", "query_only":false, "direct_writes":false,
            "rename_preview":{"apply_tool":"file_changes",
                "rust":{"versioned_document_changes_required":true},
                "typescript":{"native_changes":"request_snapshot_bound","document_changes":"strict_versions_required"},
                "max_files":super::file_changes::MAX_FILES,"max_snapshot_bytes":super::file_changes::MAX_TOTAL_BYTES},
            "sandbox":self.effective_sandbox(), "sandbox_backend_available":null,
            "sandbox_backend_check":"not_checked; configuration status never starts a probe process",
            "limits":{"document_bytes":config::MAX_DOCUMENT_BYTES,"frame_bytes":transport::MAX_FRAME_BYTES,
                "results":MAX_RESULTS,"open_documents":MAX_OPEN_DOCUMENTS,"output_bytes":output::MAX_OUTPUT_BYTES},
            "boundaries":[
                "Rename only produces a versioned file_changes preview; actual writes require file_changes.apply. Server commands, resource operations, annotations and workspace/applyEdit are refused.",
                "External server code runs through command approval and the effective OS sandbox. Write/network fences do not isolate host reads.",
                "TypeScript config inheritance, project references, plugins and symlinked configuration directories are refused. Parent and nested configs are inspected; automatic type acquisition is disabled.",
                "Only supported source-file URIs inside this workspace are returned. Each opened overlay is refreshed before a query; changes to checked snapshots invalidate the result.",
                "Missing diagnostics are not clean. Unversioned publications are unverified. Rust automatic cargo checks are disabled, so compiler/build-script diagnostics may be absent."
            ]
        }).to_string())
    }
}

fn execution_fence(registry: &ToolRegistry) -> Result<String, String> {
    if registry.approval_mode() == ApprovalMode::ReadOnly || registry.delegation_read_only {
        return Err("current policy forbids language-server processes".into());
    }
    serde_json::to_string(&(
        registry.approval_mode().as_str(),
        registry.effective_sandbox(),
    ))
    .map_err(|e| format!("encode LSP execution policy: {e}"))
}

fn lsp_error(error: impl Into<String>) -> ToolError {
    ToolError::LanguageServer(error.into())
}

fn short_text(text: &str, limit: usize) -> String {
    if text.len() <= limit {
        return text.to_owned();
    }
    let mut end = limit;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &text[..end])
}

/// LSP recognizes LF, CRLF and lone CR as line endings. Keep byte offsets while
/// excluding terminators from the UTF-16 character coordinate space.
fn source_lines(text: &str) -> impl Iterator<Item = (usize, &str)> {
    let mut cursor = 0;
    std::iter::from_fn(move || {
        if cursor > text.len() {
            return None;
        }
        let start = cursor;
        let rest = &text[start..];
        if let Some(index) = rest.bytes().position(|byte| matches!(byte, b'\r' | b'\n')) {
            let end = start + index;
            let width =
                if text.as_bytes()[end] == b'\r' && text.as_bytes().get(end + 1) == Some(&b'\n') {
                    2
                } else {
                    1
                };
            cursor = end + width;
            Some((start, &text[start..end]))
        } else {
            cursor = text.len() + 1;
            Some((start, rest))
        }
    })
}

fn validate_position(text: &str, line: Option<u32>, character: Option<u32>) -> Result<(), String> {
    let line = line.ok_or("line is required (0-based)")? as usize;
    let character = character.ok_or("character is required (0-based UTF-16)")? as usize;
    let (_, row) = source_lines(text)
        .nth(line)
        .ok_or("line is outside source snapshot")?;
    let mut units = 0;
    if character == 0 {
        return Ok(());
    }
    for ch in row.chars() {
        units += ch.len_utf16();
        if units == character {
            return Ok(());
        }
        if units > character {
            return Err("character splits a UTF-16 surrogate pair".into());
        }
    }
    Err("character is outside source line (UTF-16 offsets required)".into())
}
