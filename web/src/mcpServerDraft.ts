// MCP 服务表单的草稿与提交体。与组件分开放，页面和测试共用同一套拆分规则。

import type { McpDraft } from "./McpServers";

export type { McpDraft };

export function emptyDraft(): McpDraft {
  return {
    name: "", transport: "stdio", command: "", args: "", env: "", url: "", headers: "",
    auth: "none", token: "", bearer_token_env: "", oauthClientId: "", oauthClientSecretEnv: "", oauthScopes: "",
  };
}

/// 表单是几行纯文本，接口要的是结构化字段。
export function draftToRequest(draft: McpDraft): Record<string, unknown> {
  const body: Record<string, unknown> = { name: draft.name.trim() };
  if (draft.transport === "stdio") {
    body.command = draft.command.trim();
    body.args = splitWords(draft.args);
    body.env = Object.fromEntries(lines(draft.env).map((line) => {
      const at = line.indexOf("=");
      return at < 0 ? [line, ""] : [line.slice(0, at).trim(), line.slice(at + 1)];
    }));
  } else {
    body.url = draft.url.trim();
    body.headers = Object.fromEntries(lines(draft.headers).map((line) => {
      const at = line.indexOf(":");
      return at < 0 ? [line, ""] : [line.slice(0, at).trim(), line.slice(at + 1).trim()];
    }));
    if (draft.auth === "token") body.token = draft.token;
    if (draft.auth === "bearer_token_env") body.bearer_token_env = draft.bearer_token_env.trim();
    if (draft.auth === "oauth") {
      body.oauth_client_id = draft.oauthClientId.trim() || null;
      body.oauth_client_secret_env = draft.oauthClientSecretEnv.trim() || null;
      body.oauth_scopes = splitWords(draft.oauthScopes);
    }
  }
  return body;
}

function lines(value: string) {
  return value.split("\n").map((line) => line.trim()).filter((line) => line.length > 0);
}

function splitWords(value: string) {
  return value.split(/\s+/).map((word) => word.trim()).filter((word) => word.length > 0);
}
