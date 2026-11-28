// 配置页里的 MCP 服务管理。
//
// 添加和删除都落到 `$WILLDEEP_HOME/.mcp/<name>.toml`，和 `willdeep mcp add` 写的是同一处。
// 新文件要等 runtime 下次启动才连上；凭据走环境变量时，变量必须在启动 runtime 的那个
// 环境里，启动之后再设等于没有。页面拿回来的名单里没有 token 明文，只知道写了没有。

import { useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import type { Messages } from "./i18n";
import { draftToRequest, emptyDraft } from "./mcpServerDraft";
import "./intake.css";

export type McpServerView = {
  name: string;
  transport: "stdio" | "http";
  command: string | null;
  args: string[];
  env: string[];
  url: string | null;
  headers: string[];
  auth: "none" | "token" | "bearer_token_env" | "oauth";
  token_set: boolean;
  bearer_token_env: string | null;
  oauth_client_id: string | null;
  startup_timeout_seconds: number;
  enabled: boolean;
};

export type McpServerList = { servers: McpServerView[]; skipped: string[] };

export type McpDraft = {
  name: string;
  transport: "stdio" | "http";
  command: string;
  args: string;
  env: string;
  url: string;
  headers: string;
  auth: "none" | "token" | "bearer_token_env" | "oauth";
  token: string;
  bearer_token_env: string;
  oauthClientId: string;
  oauthClientSecretEnv: string;
  oauthScopes: string;
};

const listKey = ["desktop-mcp-servers"];

export function McpServers({ messages: t }: { messages: Messages }) {
  const client = useQueryClient();
  const list = useQuery({
    queryKey: listKey,
    queryFn: ({ signal }) => readList(signal),
    retry: false,
  });
  const [draft, setDraft] = useState<McpDraft | null>(null);
  const [error, setError] = useState("");

  const add = useMutation({
    mutationFn: (body: McpDraft) => postServer(body),
    onSuccess: async () => {
      setDraft(null);
      setError("");
      await client.invalidateQueries({ queryKey: listKey });
    },
    onError: (reason: Error) => setError(reason.message),
  });
  const remove = useMutation({
    mutationFn: (name: string) => deleteServer(name),
    onSuccess: async () => {
      setError("");
      await client.invalidateQueries({ queryKey: listKey });
    },
    onError: (reason: Error) => setError(reason.message),
  });

  return <main className="mcp-page">
    <h1>{t.mcpServersTitle}</h1>
    <p>{t.mcpServersIntro}</p>
    <p className="mcp-note">{t.mcpServersEnvNote}</p>
    {list.isLoading && <p>{t.mcpServersLoading}</p>}
    {list.isError && <p role="alert">{t.mcpServersLoadFailed}</p>}
    {error && <p role="alert">{error}</p>}
    {list.data && list.data.skipped.length > 0 && <p role="alert">{t.mcpServersSkipped}: {list.data.skipped.join("; ")}</p>}
    {list.data && <ul className="cli-options">
      {list.data.servers.map((server) => (
        <li key={server.name}>
          <span className="mcp-server-summary">
            <code>{server.name}</code>
            <small>{describe(server, t)}</small>
          </span>
          <button
            type="button"
            disabled={remove.isPending}
            onClick={() => {
              if (window.confirm(t.mcpServersRemoveConfirm.replace("{name}", server.name))) remove.mutate(server.name);
            }}
          >{t.mcpServersRemove}</button>
        </li>
      ))}
      {list.data.servers.length === 0 && <li>{t.mcpServersEmpty}</li>}
    </ul>}
    {draft
      ? <ServerForm draft={draft} busy={add.isPending} messages={t} onChange={setDraft} onSubmit={() => add.mutate(draft)} onCancel={() => setDraft(null)} />
      : <button type="button" onClick={() => { setError(""); setDraft(emptyDraft()); }}>{t.mcpServersAdd}</button>}
  </main>;
}

function describe(server: McpServerView, t: Messages) {
  const where = server.transport === "http" ? server.url ?? "" : [server.command, ...server.args].filter(Boolean).join(" ");
  const auth = server.auth === "token"
    ? t.mcpServersAuthToken
    : server.auth === "bearer_token_env"
      ? t.mcpServersAuthBearer.replace("{name}", server.bearer_token_env ?? "")
      : server.auth === "oauth"
        ? t.mcpServersAuthOauth
        : t.mcpServersAuthNone;
  return `${server.transport === "http" ? t.mcpServersHttp : t.mcpServersStdio} · ${where} · ${auth}${server.enabled ? "" : ` · ${t.mcpServersDisabled}`}`;
}

function ServerForm({ draft, busy, messages: t, onChange, onSubmit, onCancel }: {
  draft: McpDraft;
  busy: boolean;
  messages: Messages;
  onChange: (draft: McpDraft) => void;
  onSubmit: () => void;
  onCancel: () => void;
}) {
  const set = (patch: Partial<McpDraft>) => onChange({ ...draft, ...patch });
  return <form className="mcp-server-form" onSubmit={(event) => { event.preventDefault(); onSubmit(); }}>
    <label>{t.mcpServersName}<input value={draft.name} required pattern="[A-Za-z0-9_-]{1,64}" onChange={(event) => set({ name: event.target.value })} /></label>
    <fieldset>
      <legend>{t.mcpServersTransport}</legend>
      <label><input type="radio" name="transport" checked={draft.transport === "stdio"} onChange={() => set({ transport: "stdio" })} />{t.mcpServersStdio}</label>
      <label><input type="radio" name="transport" checked={draft.transport === "http"} onChange={() => set({ transport: "http" })} />{t.mcpServersHttp}</label>
    </fieldset>
    {draft.transport === "stdio"
      ? <>
          <label>{t.mcpServersCommand}<input value={draft.command} required onChange={(event) => set({ command: event.target.value })} /></label>
          <label>{t.mcpServersArgs}<input value={draft.args} placeholder={t.mcpServersArgsHint} onChange={(event) => set({ args: event.target.value })} /></label>
          <label>{t.mcpServersEnv}<input value={draft.env} placeholder="NAME=value" onChange={(event) => set({ env: event.target.value })} /></label>
        </>
      : <>
          <label>{t.mcpServersUrl}<input value={draft.url} required type="url" onChange={(event) => set({ url: event.target.value })} /></label>
          <label>{t.mcpServersHeaders}<input value={draft.headers} placeholder="Name: value" onChange={(event) => set({ headers: event.target.value })} /></label>
          <p className="mcp-note">{t.mcpServersHeaderNote}</p>
          <fieldset>
            <legend>{t.mcpServersAuth}</legend>
            {(["none", "token", "bearer_token_env", "oauth"] as const).map((auth) => (
              <label key={auth}><input type="radio" name="auth" checked={draft.auth === auth} onChange={() => set({ auth })} />{authLabel(auth, t)}</label>
            ))}
          </fieldset>
          {draft.auth === "token" && <label>{t.mcpServersToken}<input type="password" value={draft.token} required autoComplete="off" onChange={(event) => set({ token: event.target.value })} /></label>}
          {draft.auth === "bearer_token_env" && <label>{t.mcpServersBearerEnv}<input value={draft.bearer_token_env} required pattern="[A-Za-z_][A-Za-z0-9_]*" onChange={(event) => set({ bearer_token_env: event.target.value })} /></label>}
          {draft.auth === "oauth" && <>
            <label>{t.mcpServersOauthClientId}<input value={draft.oauthClientId} onChange={(event) => set({ oauthClientId: event.target.value })} /></label>
            <label>{t.mcpServersOauthSecretEnv}<input value={draft.oauthClientSecretEnv} onChange={(event) => set({ oauthClientSecretEnv: event.target.value })} /></label>
            <label>{t.mcpServersOauthScopes}<input value={draft.oauthScopes} onChange={(event) => set({ oauthScopes: event.target.value })} /></label>
          </>}
        </>}
    <div className="mcp-server-actions">
      <button type="submit" disabled={busy}>{busy ? t.mcpServersSaving : t.mcpServersSave}</button>
      <button type="button" onClick={onCancel}>{t.cancel}</button>
    </div>
  </form>;
}

function authLabel(auth: McpDraft["auth"], t: Messages) {
  if (auth === "token") return t.mcpServersAuthToken;
  if (auth === "bearer_token_env") return t.mcpServersAuthBearer.replace("{name}", "");
  if (auth === "oauth") return t.mcpServersAuthOauth;
  return t.mcpServersAuthNone;
}

async function readList(signal: AbortSignal): Promise<McpServerList> {
  const response = await fetch("/api/desktop/mcp", { signal });
  if (!response.ok) throw new Error(String(response.status));
  return await response.json() as McpServerList;
}

async function postServer(draft: McpDraft): Promise<void> {
  const response = await fetch("/api/desktop/mcp", {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(draftToRequest(draft)),
  });
  if (!response.ok) {
    const value = await response.json().catch(() => ({})) as { error?: string };
    throw new Error(value.error || String(response.status));
  }
}

async function deleteServer(name: string): Promise<void> {
  const response = await fetch(`/api/desktop/mcp/${encodeURIComponent(name)}`, { method: "DELETE" });
  if (!response.ok && response.status !== 204) throw new Error(String(response.status));
}
