// MCP 接入片段的拼接。与组件分开放，组件文件只导出组件，热更新才能生效。

import { intakeBase, intakeBaseIsLoopback, type IntakeView } from "./intakeApi";

/// 安装片段用的端点。公网入口开着且配了对外域名时用它，否则退回当前页面的
/// 地址——那种情况下只有本机能连，页面要说清楚。
export function mcpEndpoint(intake: IntakeView | null): { url: string; public: boolean } {
  const base = intake && intake.running ? intakeBase(intake) : null;
  const usable = base && !intakeBaseIsLoopback(base) ? base : null;
  return { url: `${usable ?? window.location.origin}/mcp`, public: usable !== null };
}

/// 三段安装片段。token 只在刚签发时拿得到，所以命令也只在那一次拼得出来。
export function installSnippets(token: string, url: string) {
  const claudeCode = JSON.stringify(
    { mcpServers: { willdeep: { type: "http", url, headers: { Authorization: `Bearer ${token}` } } } },
    null,
    2,
  );
  // willdeep 的服务文件默认把静态 token 明文写在自己的 toml 里，与这里展示的一致。
  const willdeep = [
    "# $WILLDEEP_HOME/.mcp/willdeep.toml",
    `url = "${url}"`,
    `token = "${token}"`,
  ].join("\n");
  // Codex 不收明文 token，只收环境变量名，值在它自己的进程里读。
  const codex = [
    "# ~/.codex/config.toml",
    "[mcp_servers.willdeep]",
    `url = "${url}"`,
    "bearer_token_env_var = \"WILLDEEP_MCP_TOKEN\"",
    "",
    "# export WILLDEEP_MCP_TOKEN='<上面显示的 token>' 后再启动 Codex",
  ].join("\n");
  const chatGpt = `${url}\nAuthorization: Bearer ${token}`;
  return { claudeCode, willdeep, codex, chatGpt };
}
