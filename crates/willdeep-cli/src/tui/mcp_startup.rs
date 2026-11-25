use super::Language;
use willdeep_core::mcp::{McpError, McpStartupWarningKind};

pub(crate) fn mcp_startup_notice(
    language: Language,
    kind: McpStartupWarningKind,
    server: &str,
    error: &McpError,
) -> String {
    let message = match kind {
        McpStartupWarningKind::Unavailable => language.pick(
            format!("MCP 服务 {server:?} 启动失败，已跳过。"),
            format!("MCP server {server:?} could not start and was skipped."),
            format!("MCP サーバー {server:?} の起動に失敗したため、スキップしました。"),
        ),
        McpStartupWarningKind::ToolListUnavailable => language.pick(
            format!("无法读取 MCP 服务 {server:?} 的工具列表。"),
            format!("Could not read the tool list for MCP server {server:?}."),
            format!("MCP サーバー {server:?} のツール一覧を取得できませんでした。"),
        ),
    };
    // Remote errors can contain URLs, credentials or arbitrary response bodies.
    // Only display known local configuration details.
    let error = match error {
        McpError::Startup { source, .. } => source.as_ref(),
        error => error,
    };
    let detail = match error {
        McpError::InvalidConfig(detail) => detail
            .strip_prefix(&format!("mcp_servers.{server}: environment variable "))
            .and_then(|detail| detail.strip_suffix(" is not set"))
            .filter(|name| {
                !name.is_empty()
                    && name
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
            })
            .map(|name| {
                language.pick(
                    format!("缺少环境变量 {name}。"),
                    format!("Environment variable {name} is not set."),
                    format!("環境変数 {name} が設定されていません。"),
                )
            }),
        McpError::Unauthorized { .. } => Some(
            language
                .text(
                    "需要授权。",
                    "Authorization is required.",
                    "認証が必要です。",
                )
                .to_owned(),
        ),
        _ => None,
    };
    match detail {
        Some(detail) => format!("System: {message} {detail}"),
        None => format!("System: {message}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn startup_notice_shows_missing_env_without_rendering_remote_errors() {
        let message = mcp_startup_notice(
            Language::En,
            McpStartupWarningKind::Unavailable,
            "someapi",
            &McpError::Startup {
                server: "someapi".to_owned(),
                transport: "streamable_http",
                stage: "HTTP connection setup",
                source: Box::new(McpError::InvalidConfig(
                    "mcp_servers.someapi: environment variable TEST_MCP_TOKEN is not set"
                        .to_owned(),
                )),
            },
        );
        assert_eq!(
            message,
            "System: MCP server \"someapi\" could not start and was skipped. Environment variable TEST_MCP_TOKEN is not set."
        );
        let message = mcp_startup_notice(
            Language::En,
            McpStartupWarningKind::ToolListUnavailable,
            "remote",
            &McpError::Remote("private-response\u{1b}[2J".to_owned()),
        );
        assert_eq!(
            message,
            "System: Could not read the tool list for MCP server \"remote\"."
        );
    }
}
