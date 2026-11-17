//! Configuration discovery only. The shareable report contains no endpoints,
//! executable paths, artifact paths or credentials, and never probes a service.
use super::*;
use serde_json::Value;
use willdeep_core::{ApprovalMode, ToolCall, ToolRegistry};

pub(super) async fn check_optional_tools(options: &DoctorOptions, checks: &mut Vec<DoctorCheck>) {
    let workspace = options
        .workspace
        .clone()
        .or_else(|| std::env::current_dir().ok());
    let Some(workspace) = workspace else {
        return;
    };
    let loaded = match LoadedConfig::load(options.config_path.as_deref()) {
        Ok(loaded) => loaded,
        Err(_) => {
            checks.push(check(
                "optional_tools",
                CheckStatus::Warning,
                "configuration unavailable; run config check",
            ));
            return;
        }
    };
    let search = match loaded.file.search.resolve() {
        Ok(search) => search,
        Err(_) => {
            checks.push(check(
                "optional_tools",
                CheckStatus::Warning,
                "search configuration or environment reference unavailable; run config check",
            ));
            return;
        }
    };
    let tools = match ToolRegistry::new(&workspace, ApprovalMode::ReadOnly)
        .and_then(|tools| tools.with_browser_config(loaded.file.browser.clone()))
    {
        Ok(tools) => tools
            .with_search_settings(search)
            .with_mcp(std::sync::Arc::new(
                willdeep_core::McpRegistry::configuration_only(&loaded.file.mcp_servers),
            )),
        Err(_) => {
            checks.push(check(
                "optional_tools",
                CheckStatus::Warning,
                "workspace or optional-tool configuration unavailable; run config check",
            ));
            return;
        }
    };
    let call = ToolCall {
        id: "doctor-configuration".into(),
        name: "tool_status".into(),
        arguments: "{}".into(),
    };
    match tools.execute(&call).await.and_then(|text| {
        serde_json::from_str::<Value>(&text).map_err(|_| {
            willdeep_core::tools::ToolError::Network("invalid configuration status".into())
        })
    }) {
        Ok(value) => append_summaries(&value, checks),
        Err(_) => checks.push(check(
            "optional_tools",
            CheckStatus::Warning,
            "configuration discovery failed; no connectivity test performed",
        )),
    }
}

fn append_summaries(value: &Value, checks: &mut Vec<DoctorCheck>) {
    for probe in value["integrations"].as_array().into_iter().flatten() {
        let config = &probe["configuration"];
        let (name, summary, healthy) = match probe["tool"].as_str() {
            Some("lsp") => {
                let available = config["servers"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter(|server| server["dependency"]["available"] == true)
                    .count();
                let valid = config["servers"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter(|server| server["configuration_check"]["valid"] == true)
                    .count();
                let reasons = config["servers"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter(|server| server["configuration_check"]["valid"] != true)
                    .map(lsp_configuration_reason)
                    .collect::<std::collections::BTreeSet<_>>()
                    .into_iter()
                    .collect::<Vec<_>>()
                    .join(",");
                (
                    "lsp",
                    format!(
                        "dependencies_available={available}/2; configuration_valid={valid}/2; configuration_reason={}; run config check and provide trusted language servers if needed",
                        if reasons.is_empty() { "none" } else { &reasons }
                    ),
                    available == 2
                        && valid == 2
                        && config["servers"].as_array().is_some_and(|servers| {
                            servers.len() == 2
                                && servers.iter().all(|server| {
                                    server["dependency"]["available"] == true
                                        && server["configuration_check"]["valid"] == true
                                })
                        }),
                )
            }
            Some("ast_grep") => {
                let available = config["dependency"]["available"] == true;
                (
                    "ast_grep",
                    format!(
                        "dependency_available={available}; provide trusted ast-grep on an absolute PATH outside the workspace"
                    ),
                    available,
                )
            }
            Some("browser") => {
                let enabled = config["enabled"] == true;
                let available = config["dependencies"]["paths_available"] == true;
                (
                    "browser",
                    format!(
                        "enabled={enabled}; dependency_paths_available={available}; configure absolute Node/Chrome paths, artifact directory and allowed origins"
                    ),
                    !enabled || available,
                )
            }
            Some("web_search") => {
                let state = match config["state"].as_str() {
                    Some("configured") => "configured",
                    Some("disabled") => "disabled",
                    Some("not_configured") => "not_configured",
                    _ => "invalid_configuration",
                };
                (
                    "web_search",
                    format!(
                        "state={state}; configure independent [search] or select compatible SomeIm at Runtime startup"
                    ),
                    state != "invalid_configuration",
                )
            }
            Some("mcp") => {
                let servers = config["servers"].as_array();
                let configured = servers.map_or(0, Vec::len);
                let enabled = servers
                    .into_iter()
                    .flatten()
                    .filter(|server| server["enabled"] == true)
                    .count();
                let invalid = servers
                    .into_iter()
                    .flatten()
                    .filter(|server| server["configuration_check"]["valid"] != true)
                    .count();
                (
                    "mcp",
                    format!(
                        "configured={configured}; enabled={enabled}; invalid_configuration={invalid}; authentication=unknown; use mcp list and explicitly connect or log in if needed"
                    ),
                    invalid == 0,
                )
            }
            _ => continue,
        };
        checks.push(check(name, if healthy { CheckStatus::Pass } else { CheckStatus::Warning },
            format!("{summary}; connectivity=not_checked; runtime_permissions=not_checked; no processes started")));
    }
}

// Only host-owned labels enter a shareable report, never configuration errors.
fn lsp_configuration_reason(server: &Value) -> &'static str {
    match server["configuration_check"]["reason"].as_str() {
        Some(reason) if reason.contains("extends") => "inheritance_disabled",
        Some(reason) if reason.contains("timed out") => "inspection_timed_out",
        Some(_) => "configuration_rejected",
        None => "not_verified",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn summaries_never_copy_untrusted_paths_endpoints_or_messages() {
        let secret = "synthetic-sensitive-marker";
        let value = json!({"integrations":[
            {"tool":"lsp","configuration":{"servers":[{"dependency":{"available":false,"reason":secret}}]}},
            {"tool":"ast_grep","configuration":{"dependency":{"available":false,"executable":secret}}},
            {"tool":"browser","configuration":{"enabled":true,"artifact_dir":secret,"dependencies":{"paths_available":false,"reason":secret}}},
            {"tool":"web_search","configuration":{"state":secret,"endpoint":secret,"message":secret}},
            {"tool":"mcp","configuration":{"servers":[{"name":secret,"url":secret,"enabled":true,"configuration_check":{"valid":false,"reason":secret},"authentication":{"state":secret}}]}}
        ]});
        let mut checks = Vec::new();
        append_summaries(&value, &mut checks);
        let text = serde_json::to_string(&checks).unwrap();
        assert_eq!(checks.len(), 5);
        assert!(!text.contains(secret));
        assert!(
            checks
                .iter()
                .all(|check| check.status == CheckStatus::Warning)
        );
        assert!(text.contains("connectivity=not_checked"));
    }

    #[test]
    fn installed_lsp_dependencies_do_not_hide_invalid_or_missing_configuration() {
        for (configuration, label) in [
            (
                json!({"valid":false,"reason":"TypeScript config inheritance (extends) is disabled: /private/never-copy.json"}),
                "inheritance_disabled",
            ),
            (
                json!({"valid":false,"reason":"invalid JSON at https://private.invalid/never-copy"}),
                "configuration_rejected",
            ),
            (
                json!({"valid":false,"reason":"configuration inspection timed out"}),
                "inspection_timed_out",
            ),
            (json!({}), "not_verified"),
        ] {
            let value = json!({"integrations":[{"tool":"lsp","configuration":{"servers":[
                {"dependency":{"available":true},"configuration_check":{"valid":true}},
                {"dependency":{"available":true},"configuration_check":configuration}
            ]}}]});
            let mut checks = Vec::new();
            append_summaries(&value, &mut checks);
            assert_eq!(checks.len(), 1);
            assert_eq!(checks[0].status, CheckStatus::Warning);
            let text = serde_json::to_string(&checks).unwrap();
            assert!(text.contains("dependencies_available=2/2"));
            assert!(text.contains("configuration_valid=1/2"));
            assert!(text.contains(label));
            assert!(!text.contains("never-copy"));
            assert!(!text.contains("private.invalid"));
        }
    }

    #[test]
    fn lsp_pass_requires_complete_valid_dependency_and_configuration_pairs() {
        let valid = json!({"dependency":{"available":true},"configuration_check":{"valid":true}});
        for (servers, expected) in [
            (json!([valid.clone(), valid.clone()]), CheckStatus::Pass),
            (json!([valid.clone()]), CheckStatus::Warning),
            (
                json!([
                    valid.clone(),
                    {"dependency":{"available":true},"configuration_check":{"valid":false}},
                    {"dependency":{"available":false},"configuration_check":{"valid":true}}
                ]),
                CheckStatus::Warning,
            ),
        ] {
            let value =
                json!({"integrations":[{"tool":"lsp","configuration":{"servers":servers}}]});
            let mut checks = Vec::new();
            append_summaries(&value, &mut checks);
            assert_eq!(checks[0].status, expected);
        }
    }

    #[test]
    fn mcp_summary_distinguishes_configured_enabled_and_invalid() {
        let value = json!({"integrations":[{"tool":"mcp","configuration":{"servers":[
            {"enabled":false,"configuration_check":{"valid":true}},
            {"enabled":true,"configuration_check":{"valid":true}},
            {"enabled":true,"configuration_check":{"valid":false}}
        ]}}]});
        let mut checks = Vec::new();
        append_summaries(&value, &mut checks);
        assert_eq!(checks[0].status, CheckStatus::Warning);
        let text = serde_json::to_string(&checks).unwrap();
        assert!(text.contains("configured=3; enabled=2; invalid_configuration=1"));
        assert!(text.contains("authentication=unknown"));
    }
}
