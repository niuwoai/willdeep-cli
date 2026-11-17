//! The only LSP client implementation. The outer tool deadline owns this client;
//! dropping an unfinished query drops Rpc and kills its entire process tree.
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;

use serde_json::{Value, json};

use super::config::{Language, Launch, read_document, snapshot, uri};
use super::transport::Rpc;
use super::{Arguments, MAX_OPEN_DOCUMENTS, Operation};
use crate::sandbox::SandboxSpec;

#[derive(Clone, Debug)]
pub(super) struct Document {
    pub(super) path: PathBuf,
    pub(super) uri: String,
    pub(super) version: i64,
    pub(super) snapshot: String,
}

pub(super) struct QueryContext {
    pub(super) document: Document,
    pub(super) text: String,
    pub(super) checked_documents: Vec<Document>,
}

pub(super) struct QueryPayload {
    pub(super) raw: Value,
    pub(super) diagnostics_mode: Option<&'static str>,
    pub(super) published_version: Option<i64>,
    pub(super) version_verified: bool,
    pub(super) truncated: bool,
}

pub(super) struct Client {
    pub(super) rpc: Rpc,
    pub(super) capabilities: Value,
    pub(super) documents: BTreeMap<String, Document>,
    pub(super) profile: String,
    language: Language,
    next_version: i64,
}

impl Client {
    pub(super) async fn start(
        root: &Path,
        language: Language,
        sandbox: &SandboxSpec,
        profile: String,
        launch: Launch,
    ) -> Result<Self, String> {
        let root_uri = uri(root)?;
        let mut command = crate::execution::shell(&launch.command, sandbox)
            .map_err(|e| format!("cannot construct LSP sandbox: {e}"))?;
        command
            .current_dir(root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .env("PATH", &launch.path)
            // Environment hooks run before initialization settings take effect.
            .env_remove("NODE_OPTIONS")
            .env_remove("NODE_PATH")
            .env_remove("RUSTC_WRAPPER")
            .env_remove("RUSTC_WORKSPACE_WRAPPER")
            .env_remove("BASH_ENV")
            .env_remove("ENV");
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.as_std_mut().process_group(0);
        }
        #[cfg(windows)]
        let job = crate::execution::windows_job::Job::new(&mut command)
            .map_err(|e| format!("cannot create LSP process job: {e}"))?;
        let mut child = command.spawn().map_err(|e| {
            format!(
                "cannot start {}: {e}. {}",
                language.command(),
                language.installation_hint()
            )
        })?;
        #[cfg(windows)]
        job.attach_and_resume(&child)
            .map_err(|e| format!("cannot isolate LSP process job: {e}"))?;
        let stdin = child
            .stdin
            .take()
            .ok_or("language server stdin is unavailable")?;
        let stdout = child
            .stdout
            .take()
            .ok_or("language server stdout is unavailable")?;
        let stderr = child
            .stderr
            .take()
            .ok_or("language server stderr is unavailable")?;
        let mut rpc = Rpc::from_io(stdout, stdin, language, root_uri);
        #[cfg(windows)]
        rpc.attach_process(child, stderr, job);
        #[cfg(not(windows))]
        rpc.attach_process(child, stderr);
        Self::initialize(rpc, root, language, profile, launch.options).await
    }

    // from_io exercises the same initialization and query code in offline tests.
    pub(super) async fn initialize(
        mut rpc: Rpc,
        root: &Path,
        language: Language,
        profile: String,
        options: Value,
    ) -> Result<Self, String> {
        let root_uri = uri(root)?;
        let mut params = json!({
            "processId":std::process::id(), "clientInfo":{"name":"willdeep"},
            "rootUri":root_uri, "workspaceFolders":[{"uri":root_uri,"name":"workspace"}],
            "capabilities":{
                "general":{"positionEncodings":["utf-16"]},
                "workspace":{"applyEdit":false,"configuration":true,"workspaceFolders":true,
                    "workspaceEdit":{"documentChanges":true,"resourceOperations":[],"failureHandling":"abort"},
                    "didChangeWatchedFiles":{"dynamicRegistration":false}},
                "textDocument":{
                    "synchronization":{"dynamicRegistration":false,"didSave":false},
                    "definition":{"dynamicRegistration":false,"linkSupport":true},
                    "references":{"dynamicRegistration":false},
                    "rename":{"dynamicRegistration":false,"prepareSupport":false,"honorsChangeAnnotations":false},
                    "documentSymbol":{"dynamicRegistration":false,"hierarchicalDocumentSymbolSupport":true},
                    "publishDiagnostics":{"versionSupport":true,"relatedInformation":false},
                    "diagnostic":{"dynamicRegistration":false,"relatedDocumentSupport":false}
                }
            }, "initializationOptions":options, "trace":"off"
        });
        if language == Language::Rust {
            params["capabilities"]["experimental"] = json!({"serverStatusNotification":true});
        }
        let initialization = rpc.request("initialize", params).await.map_err(|e| {
            format!(
                "initialize {} failed: {e}. {}",
                language.name(),
                language.installation_hint()
            )
        })?;
        // This profile's readiness contract is rust-analyzer-specific. Do not
        // accept a different Rust server and wait for a protocol it may not use.
        if language == Language::Rust
            && initialization
                .pointer("/serverInfo/name")
                .and_then(Value::as_str)
                != Some("rust-analyzer")
        {
            return Err("Rust LSP requires rust-analyzer serverInfo and experimental/serverStatus readiness notifications; this server is unsupported".into());
        }
        let capabilities = initialization
            .get("capabilities")
            .filter(|v| v.is_object())
            .cloned()
            .ok_or("language server initialize result has no capabilities")?;
        if capabilities
            .get("positionEncoding")
            .and_then(Value::as_str)
            .is_some_and(|value| value != "utf-16")
        {
            return Err(
                "language server selected unsupported position encoding; UTF-16 is required".into(),
            );
        }
        let sync = &capabilities["textDocumentSync"];
        let change = sync.as_u64().or_else(|| sync["change"].as_u64());
        if !matches!(change, Some(1 | 2))
            || sync.get("openClose").and_then(Value::as_bool) == Some(false)
        {
            return Err("language server lacks document open/change synchronization required for versioned queries".into());
        }
        rpc.notify("initialized", json!({})).await?;
        let settings = match language {
            Language::Rust => json!({"rust-analyzer":language.configuration()}),
            Language::TypeScript => language.configuration(),
        };
        rpc.notify(
            "workspace/didChangeConfiguration",
            json!({"settings":settings}),
        )
        .await?;
        let client = Self {
            rpc,
            capabilities,
            documents: BTreeMap::new(),
            profile,
            language,
            next_version: 1,
        };
        client.wait_until_ready().await?;
        Ok(client)
    }

    pub(super) async fn wait_until_ready(&self) -> Result<Option<u64>, String> {
        match self.language {
            Language::Rust => self.rpc.rust_ready().await.map(Some),
            Language::TypeScript => Ok(None),
        }
    }

    pub(super) async fn verify_readiness(&self, revision: Option<u64>) -> Result<(), String> {
        if self.wait_until_ready().await? != revision {
            return Err("rust-analyzer project readiness changed during query; stale result discarded; retry when project loading completes".into());
        }
        Ok(())
    }

    pub(super) fn language(&self) -> Language {
        self.language
    }

    pub(super) fn supports(&self, capability: &str) -> bool {
        self.capabilities
            .get(capability)
            .is_some_and(|value| value == &Value::Bool(true) || value.is_object())
    }

    pub(super) fn capability_summary(&self) -> Value {
        json!({
            "definition":self.supports("definitionProvider"),
            "references":self.supports("referencesProvider"),
            "document_symbols":self.supports("documentSymbolProvider"),
            "workspace_symbols":self.supports("workspaceSymbolProvider"),
            "pull_diagnostics":self.supports("diagnosticProvider"),
            "position_encoding":"utf-16", "rename_preview":self.supports("renameProvider"),"apply_edit":false
        })
    }

    /// Refresh every open overlay, including files other than the requested one.
    /// Every synchronization advances a version, even for identical text: a push
    /// diagnostic for a previous cross-file state cannot verify this query.
    pub(super) async fn refresh_documents(
        &mut self,
        root: &Path,
        path: &Path,
    ) -> Result<QueryContext, String> {
        let mut checked = Vec::new();
        let previous: Vec<_> = self.documents.values().cloned().collect();
        for document in previous {
            if document.path == path {
                continue;
            }
            match tokio::fs::canonicalize(&document.path).await {
                Ok(real) if real == document.path && real.starts_with(root) => {
                    let text = read_document(&real).await?;
                    checked.push(self.synchronize(&real, &text).await?);
                }
                // Closing removed/replaced paths removes stale overlays; an
                // outside symlink is never read or sent to the server.
                Ok(_) => {
                    self.close(&document.uri).await?;
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    self.close(&document.uri).await?;
                }
                Err(error) => return Err(format!("recheck opened LSP document: {error}")),
            }
        }
        let real = tokio::fs::canonicalize(path)
            .await
            .map_err(|e| format!("resolve queried source: {e}"))?;
        if real != path || !real.starts_with(root) {
            return Err("queried source path changed or left workspace; retry".into());
        }
        let text = read_document(path).await?;
        let document = self.synchronize(path, &text).await?;
        checked.push(document.clone());
        Ok(QueryContext {
            document,
            text,
            checked_documents: checked,
        })
    }

    pub(super) async fn close(&mut self, uri: &str) -> Result<(), String> {
        self.rpc
            .notify("textDocument/didClose", json!({"textDocument":{"uri":uri}}))
            .await?;
        self.documents.remove(uri);
        self.rpc.forget_document(uri);
        Ok(())
    }

    pub(super) async fn synchronize(
        &mut self,
        path: &Path,
        text: &str,
    ) -> Result<Document, String> {
        let raw_uri = uri(path)?;
        if !self.documents.contains_key(&raw_uri) && self.documents.len() >= MAX_OPEN_DOCUMENTS {
            let evict = self
                .documents
                .keys()
                .next()
                .cloned()
                .expect("nonempty documents");
            self.close(&evict).await?;
        }
        let version = self.next_version;
        self.next_version = version
            .checked_add(1)
            .filter(|next| *next <= i32::MAX as i64)
            .ok_or("LSP document version exhausted; restart required")?;
        self.rpc.track_document(&raw_uri, true)?;
        if self.documents.contains_key(&raw_uri) {
            self.rpc.notify("textDocument/didChange", json!({
                "textDocument":{"uri":raw_uri,"version":version}, "contentChanges":[{"text":text}]
            })).await?;
        } else {
            self.rpc.notify("textDocument/didOpen", json!({"textDocument":{
                "uri":raw_uri,"languageId":self.language.document_language(path),"version":version,"text":text
            }})).await?;
        }
        let document = Document {
            path: path.to_owned(),
            uri: raw_uri.clone(),
            version,
            snapshot: snapshot(text.as_bytes()),
        };
        self.documents.insert(raw_uri, document.clone());
        Ok(document)
    }

    pub(super) async fn query(
        &mut self,
        args: &Arguments,
        document: &Document,
    ) -> Result<QueryPayload, String> {
        let revision = self.wait_until_ready().await?;
        let payload = self.query_ready(args, document).await?;
        // A status failure or a new loading cycle while the response was in
        // flight must not turn a partial/empty response into a clean result.
        self.verify_readiness(revision).await?;
        Ok(payload)
    }

    async fn query_ready(
        &mut self,
        args: &Arguments,
        document: &Document,
    ) -> Result<QueryPayload, String> {
        if args.operation == Operation::Diagnostics {
            if self.supports("diagnosticProvider") {
                let mut params = json!({"textDocument":{"uri":document.uri}});
                if let Some(identifier) = self
                    .capabilities
                    .pointer("/diagnosticProvider/identifier")
                    .and_then(Value::as_str)
                {
                    params["identifier"] = json!(identifier);
                }
                let report = self.rpc.request("textDocument/diagnostic", params).await?;
                if report["kind"] != "full" || !report["items"].is_array() {
                    return Err(
                        "LSP pull diagnostics omitted a full report; no previous resultId was sent"
                            .into(),
                    );
                }
                return Ok(QueryPayload {
                    raw: report["items"].clone(),
                    diagnostics_mode: Some("pull"),
                    published_version: None,
                    version_verified: true,
                    truncated: false,
                });
            }
            let publication = self
                .rpc
                .publication(&document.uri, document.version)
                .await?;
            return Ok(QueryPayload {
                raw: Value::Array(publication.diagnostics),
                diagnostics_mode: Some("push"),
                published_version: publication.version,
                version_verified: publication.version == Some(document.version),
                truncated: publication.truncated,
            });
        }
        let (method, capability, params) = match args.operation {
            Operation::Definition => (
                "textDocument/definition",
                "definitionProvider",
                json!({"textDocument":{"uri":document.uri},"position":{"line":args.line,"character":args.character}}),
            ),
            Operation::References => (
                "textDocument/references",
                "referencesProvider",
                json!({"textDocument":{"uri":document.uri},"position":{"line":args.line,"character":args.character},"context":{"includeDeclaration":true}}),
            ),
            Operation::Symbols if args.query.is_some() => (
                "workspace/symbol",
                "workspaceSymbolProvider",
                json!({"query":args.query}),
            ),
            Operation::Symbols => (
                "textDocument/documentSymbol",
                "documentSymbolProvider",
                json!({"textDocument":{"uri":document.uri}}),
            ),
            _ => return Err("unsupported LSP operation".into()),
        };
        if !self.supports(capability) {
            return Err(format!(
                "language server does not advertise {capability} for {}",
                args.operation.name()
            ));
        }
        let raw = self.rpc.request(method, params).await?;
        Ok(QueryPayload {
            raw,
            diagnostics_mode: None,
            published_version: None,
            version_verified: true,
            truncated: false,
        })
    }
}

pub(super) async fn verify_snapshots(root: &Path, documents: &[Document]) -> Result<(), String> {
    for document in documents {
        let real = tokio::fs::canonicalize(&document.path)
            .await
            .map_err(|e| format!("source changed during LSP query; stale result discarded: {e}"))?;
        if real != document.path
            || !real.starts_with(root)
            || snapshot(read_document(&real).await?.as_bytes()) != document.snapshot
        {
            return Err("source changed during LSP query; stale result discarded; retry using current files".into());
        }
    }
    Ok(())
}
