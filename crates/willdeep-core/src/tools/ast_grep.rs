//! Fixed ast-grep stdin queries. The host selects and snapshots source files;
//! the child never discovers workspace paths, configurations or parser plugins.
use std::time::Duration;

use serde::Deserialize;
use serde_json::{Value, json};

use super::{ApprovalMode, ToolDefinition, ToolError, ToolRegistry};
use launch::{IsolatedConfig, executable, query_arguments};
use snapshot::{Language, collect};

mod launch;
mod output;
mod snapshot;
#[cfg(test)]
mod tests;

const MAX_PATTERN_BYTES: usize = 4096;
const DEFAULT_RESULTS: usize = 60;
const MAX_RESULTS: usize = 200;
const DEFAULT_TIMEOUT_SECONDS: u64 = 20;
const MAX_TIMEOUT_SECONDS: u64 = 60;
const MAX_OUTPUT_BYTES: usize = 1024 * 1024;
const MAX_RESULT_BYTES: usize = 128 * 1024;

#[derive(Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum Operation {
    Status,
    Query,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Arguments {
    operation: Operation,
    path: Option<String>,
    language: Option<Language>,
    pattern: Option<String>,
    limit: Option<usize>,
    timeout_seconds: Option<u64>,
}

impl Arguments {
    fn parse(value: Value) -> Result<Self, ToolError> {
        let args: Self =
            serde_json::from_value(value).map_err(|source| ToolError::InvalidArguments {
                tool: "ast_grep".into(),
                source,
            })?;
        if args.operation == Operation::Status {
            if args.path.is_some()
                || args.language.is_some()
                || args.pattern.is_some()
                || args.limit.is_some()
                || args.timeout_seconds.is_some()
            {
                return Err(error(
                    "status accepts only operation and never starts a process",
                ));
            }
        } else {
            if args.language.is_none() {
                return Err(error("query requires language: rust, typescript or tsx"));
            }
            if args.pattern.as_ref().is_none_or(|p| {
                p.trim().is_empty() || p.len() > MAX_PATTERN_BYTES || p.contains('\0')
            }) {
                return Err(error(
                    "query pattern must contain 1..4096 UTF-8 bytes and no NUL",
                ));
            }
            if !(1..=MAX_RESULTS).contains(&args.limit.unwrap_or(DEFAULT_RESULTS)) {
                return Err(error("limit must be between 1 and 200"));
            }
            if !(1..=MAX_TIMEOUT_SECONDS)
                .contains(&args.timeout_seconds.unwrap_or(DEFAULT_TIMEOUT_SECONDS))
            {
                return Err(error("timeout_seconds must be between 1 and 60"));
            }
        }
        Ok(args)
    }
}

pub(super) fn definition() -> ToolDefinition {
    ToolDefinition {
        name: "ast_grep".into(),
        description: "Read-only structural AST search with a preinstalled ast-grep executable. status inspects dependency without spawning. query supports built-in Rust, TypeScript and TSX, host-selected regular workspace files and UTF-8 byte ranges bound to SHA-256 snapshots. Uses fixed run --stdin --json with an explicit private empty config, command approval and the effective sandbox. No rewriting, config discovery, custom parsers, shell arguments or installation. Budgets fail closed; external locations and stale snapshots are rejected.".into(),
        parameters: json!({
            "type":"object", "additionalProperties":false,
            "required":["operation"],
            "properties":{
                "operation":{"type":"string","enum":["status","query"]},
                "path":{"type":"string","description":"query only: workspace-relative source file or directory; default '.'. Symlinks, hidden/build/dependency directories and special files are rejected or excluded."},
                "language":{"type":"string","enum":["rust","typescript","tsx"]},
                "pattern":{"type":"string","minLength":1,"maxLength":MAX_PATTERN_BYTES},
                "limit":{"type":"integer","minimum":1,"maximum":MAX_RESULTS},
                "timeout_seconds":{"type":"integer","minimum":1,"maximum":MAX_TIMEOUT_SECONDS}
            }
        }),
    }
}

impl ToolRegistry {
    // No registry state or constructor changes: every call leases its config and
    // process group, and uses the registry's live approval/sandbox/deadline.
    pub(super) async fn ast_grep(&self, arguments: Value) -> Result<String, ToolError> {
        let args = Arguments::parse(arguments)?;
        let executable = executable(&self.workspace);
        if args.operation == Operation::Status {
            let (state, path, reason) = match executable {
                Ok(path) => ("available_unprobed", Some(path), None),
                Err(reason) => ("dependency_missing", None, Some(reason)),
            };
            return Ok(json!({
                "operation":"status", "state":state,
                "dependency":{"name":"ast-grep","available":path.is_some(),"executable":path,"reason":reason},
                "process_started":false,
                "languages":["rust","typescript","tsx"],
                "configuration_isolation":{
                    "mode":"explicit_private_empty_config",
                    "config_discovery":false,"dynamic_parsers":false,"rewrites":false,
                    "basis":"documented --config contract; no executable capability probe performed",
                    "unsupported_cli":"query fails closed; no fallback to discovered configuration"
                },
                "limits":snapshot::limits(),
                "recovery":"Provide a trusted native ast-grep executable on an absolute PATH directory outside the workspace. No dependency is installed automatically."
            }).to_string());
        }
        let executable = executable.map_err(error)?;
        let duration = Duration::from_secs(args.timeout_seconds.unwrap_or(DEFAULT_TIMEOUT_SECONDS));
        let duration = self
            .remaining_execution_time()
            .map_or(duration, |remaining| remaining.min(duration));
        if duration.is_zero() {
            return Err(error("query execution budget expired before starting"));
        }
        tokio::time::timeout(duration, self.ast_grep_query(args, &executable))
            .await
            .map_err(|_| error("query timed out; process group cancelled and result discarded"))?
    }

    async fn ast_grep_query(
        &self,
        args: Arguments,
        executable: &std::path::Path,
    ) -> Result<String, ToolError> {
        let language = args.language.expect("validated query language");
        let pattern = args.pattern.as_deref().expect("validated query pattern");
        let path = args.path.as_deref().unwrap_or(".");
        let limit = args.limit.unwrap_or(DEFAULT_RESULTS);
        let snapshots = collect(&self.workspace, path, language)?;
        let config = IsolatedConfig::new()?;
        let argv = query_arguments(&config, language, pattern)?;
        // Include the actual fixed executable/config/pattern in the gate, not a
        // harmless-looking alias. Argv is executed directly, never reconstructed
        // from this display string when the fence is disabled.
        let command = launch::display_command(executable, &argv)?;
        let fence = self.ast_grep_execution_fence()?;
        let recheck_policy = || {
            if self.ast_grep_execution_fence()? != fence {
                return Err(error(
                    "execution policy changed during query; discard and retry",
                ));
            }
            Ok(())
        };
        self.gate_command(
            &command,
            &format!(
                "Query {} {} source snapshots with ast-grep",
                snapshots.len(),
                language.name()
            ),
        )
        .await?;
        // The gate can await human/judge approval, and worker command grants may
        // bypass its ordinary mode check. Neither freezes the live policy.
        recheck_policy()?;
        let mut matches = Vec::new();
        let mut total_matches = 0usize;
        for snapshot in &snapshots {
            config.verify()?;
            snapshot.verify(&self.workspace)?;
            recheck_policy()?;
            let captured = crate::execution::run_capture_stdin(
                executable,
                &argv,
                config.directory(),
                &self.effective_sandbox(),
                snapshot.text.as_bytes(),
                Duration::from_secs(MAX_TIMEOUT_SECONDS),
                MAX_OUTPUT_BYTES,
            )
            .await
            .map_err(|e| error(format!("query {}: {e}", snapshot.relative)))?;
            recheck_policy()?;
            let validated = output::normalize(&captured, snapshot)?;
            total_matches = total_matches
                .checked_add(validated.len())
                .ok_or_else(|| error("match count overflow"))?;
            let room = limit.saturating_sub(matches.len());
            matches.extend(validated.into_iter().take(room));
        }
        config.verify()?;
        // Re-enumeration catches additions/removals as well as changed bytes,
        // including files that did not produce a match.
        let observed = collect(&self.workspace, path, language)?;
        if snapshots.len() != observed.len()
            || snapshots
                .iter()
                .zip(&observed)
                .any(|(before, after)| !before.same(after))
        {
            return Err(error(
                "snapshot_changed: selected source set changed during query; discard and retry",
            ));
        }
        let result = json!({
            "operation":"query", "language":language.name(),
            "state":if total_matches == 0 {"no_matches"} else {"matched"},
            "results":matches,"total_matches":total_matches,"truncated":total_matches > limit,
            "files_scanned":snapshots.len(),"snapshot_verified":true,
            "snapshots":snapshots.iter().map(|s| json!({"path":s.relative,"hash":s.hash})).collect::<Vec<_>>(),
            "range_encoding":"utf8_bytes", "config_discovery":false,
            "excluded":"hidden, build and dependency directories; symlinks and non-regular entries"
        }).to_string();
        if result.len() > MAX_RESULT_BYTES {
            return Err(error(
                "normalized result exceeded 128 KiB; narrow the path or lower limit",
            ));
        }
        recheck_policy()?;
        Ok(result)
    }

    fn ast_grep_execution_fence(&self) -> Result<String, ToolError> {
        self.check_tool_policy("ast_grep")?;
        if self.approval_mode() == ApprovalMode::ReadOnly || self.delegation_read_only {
            return Err(ToolError::ReadOnlyPolicy("ast-grep process".into()));
        }
        serde_json::to_string(&(self.approval_mode().as_str(), self.effective_sandbox()))
            .map_err(|source| error(format!("encode execution policy: {source}")))
    }
}

fn error(message: impl std::fmt::Display) -> ToolError {
    ToolError::Io(std::io::Error::other(format!("ast_grep: {message}")))
}
