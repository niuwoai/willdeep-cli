// Web 界面上的 MCP 接入管理。
//
// 一台 willdeep 除了命令行，还在 Web 服务上挂了一个 MCP 端点（POST /mcp，
// Streamable HTTP）。另一台机器上的 Claude Code、ChatGPT 拿着 token 就能来
// 读这台的会话和任务。地址用的是当前页面的协议和主机名：你从哪个地址打开的
// 这个界面，客户端就连哪个地址。
//
// 同一份内容有两个外壳：一级入口里的整页（McpPage），和侧栏齿轮里原来的
// 对话框（McpAccess）。token 的读写逻辑只在 McpBody 里。

import { useEffect, useState } from "react";
import { Box, Button, Dialog, Flex, Input, Portal, Text } from "@chakra-ui/react";
import type { Messages } from "./i18n";
import { intakeBase, intakeBaseIsLoopback, loadIntake, type IntakeView } from "./intakeApi";
import "./intake.css";

type TokenRecord = {
  id: string;
  label: string;
  created_at: number;
  revoked_at?: number | null;
};

type Issued = { id: string; label: string; token: string };

const control = { color: "var(--text)", bg: "var(--bg-raised)", borderColor: "var(--border-strong)" };

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

export function McpAccess({ messages: t, open, onOpenChange }: { messages: Messages; open: boolean; onOpenChange: (open: boolean) => void }) {
  return (
    <Dialog.Root open={open} onOpenChange={(details) => onOpenChange(details.open)} size="lg">
      <Portal>
        <Dialog.Backdrop bg="var(--shadow-strong)" />
        <Dialog.Positioner>
          <Dialog.Content bg="var(--bg-surface)" color="var(--text)" border="1px solid" borderColor="var(--bg-elevated)" borderRadius="12px">
            <Dialog.Header><Dialog.Title>{t.mcpTitle}</Dialog.Title></Dialog.Header>
            <Dialog.Body>
              <Text fontSize="sm" color="var(--text-dim)" mb="3">{t.mcpIntro}</Text>
              <McpBody messages={t} active={open} />
            </Dialog.Body>
          </Dialog.Content>
        </Dialog.Positioner>
      </Portal>
    </Dialog.Root>
  );
}

/// 一级入口里的 MCP 接入页。和对话框共用同一套 token 管理。
export function McpPage({ messages: t }: { messages: Messages }) {
  return <main className="mcp-page">
    <h1>{t.mcpTitle}</h1>
    <p>{t.mcpIntro}</p>
    <Box className="mcp-body"><McpBody messages={t} /></Box>
  </main>;
}

function McpBody({ messages: t, active = true }: { messages: Messages; active?: boolean }) {
  const [tokens, setTokens] = useState<TokenRecord[]>([]);
  const [label, setLabel] = useState("");
  const [issued, setIssued] = useState<Issued | null>(null);
  const [error, setError] = useState("");
  const [copied, setCopied] = useState("");
  const [intake, setIntake] = useState<IntakeView | null>(null);

  // 对话框每打开一次刷新一遍；整页挂载时 active 恒为 true，只拉一次。
  useEffect(() => {
    if (!active) return;
    let cancelled = false;
    void fetch("/api/mcp/tokens")
      .then((response) => response.json())
      .then((value: TokenRecord[]) => { if (!cancelled) setTokens(value); })
      .catch((reason: unknown) => { if (!cancelled) setError(String(reason)); });
    // 公网入口读不到（比如不在本机打开）就退回当前页面的地址，不挡住 token 管理。
    void loadIntake().then(
      (view) => { if (!cancelled) setIntake(view); },
      () => { if (!cancelled) setIntake(null); },
    );
    return () => { cancelled = true; };
  }, [active]);

  async function createToken() {
    setError("");
    const response = await fetch("/api/mcp/tokens", {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ label: label.trim() || "mcp" }),
    });
    const value = await response.json() as Issued & { error?: string };
    if (!response.ok) { setError(value.error || response.statusText); return; }
    setIssued(value);
    setLabel("");
    setTokens((current) => [{ id: value.id, label: value.label, created_at: Math.floor(Date.now() / 1000) }, ...current]);
  }

  async function revoke(id: string) {
    const response = await fetch(`/api/mcp/tokens/${id}`, { method: "DELETE" });
    if (!response.ok) { setError(t.mcpRevokeFailed); return; }
    setTokens((current) => current.map((token) => token.id === id ? { ...token, revoked_at: Math.floor(Date.now() / 1000) } : token));
    if (issued?.id === id) setIssued(null);
  }

  async function copy(text: string, which: string) {
    try {
      await navigator.clipboard.writeText(text);
      setCopied(which);
    } catch {
      setError(t.mcpCopyFailed);
    }
  }

  const endpoint = mcpEndpoint(intake);
  const snippets = issued ? installSnippets(issued.token, endpoint.url) : null;
  return <>
    <Text fontSize="sm" color="var(--text-dim)" mb="3">{t.mcpDirection}</Text>
    {error && <Text role="alert" color="var(--danger-text)" mb="3">{error}</Text>}
    <Flex gap="2" mb="4">
      <Input value={label} placeholder={t.mcpLabelPlaceholder} onChange={(event) => setLabel(event.target.value)} {...control} />
      <Button onClick={() => void createToken()}>{t.mcpCreate}</Button>
    </Flex>
    {snippets && (
      <Box className="mcp-issued" mb="4">
        <Text fontSize="sm" color="var(--danger-text)" mb="2">{t.mcpShownOnce}</Text>
        <Text fontSize="xs" color="var(--text-dim)" mb="1">{t.mcpEndpoint}</Text>
        <Text fontSize="sm" mb="1">{endpoint.url}</Text>
        <Text fontSize="xs" color="var(--text-dim)" mb="3">{endpoint.public ? t.mcpPublicHint : t.mcpLocalHint}</Text>
        <Snippet label={t.mcpClaudeCode} text={snippets.claudeCode} copied={copied === "claude"} onCopy={() => void copy(snippets.claudeCode, "claude")} copyLabel={t.mcpCopy} copiedLabel={t.mcpCopied} />
        <Snippet label={t.mcpWilldeep} text={snippets.willdeep} copied={copied === "willdeep"} onCopy={() => void copy(snippets.willdeep, "willdeep")} copyLabel={t.mcpCopy} copiedLabel={t.mcpCopied} />
        <Snippet label={t.mcpCodex} text={snippets.codex} copied={copied === "codex"} onCopy={() => void copy(snippets.codex, "codex")} copyLabel={t.mcpCopy} copiedLabel={t.mcpCopied} />
        <Snippet label={t.mcpChatGpt} text={snippets.chatGpt} copied={copied === "chatgpt"} onCopy={() => void copy(snippets.chatGpt, "chatgpt")} copyLabel={t.mcpCopy} copiedLabel={t.mcpCopied} />
      </Box>
    )}
    {tokens.map((token) => (
      <Flex key={token.id} justify="space-between" align="center" py="2" borderTop="1px solid" borderColor="var(--border)">
        <Text fontSize="sm" color={token.revoked_at ? "var(--text-faint)" : "var(--text)"}>{token.label}{token.revoked_at ? ` · ${t.mcpRevoked}` : ""}</Text>
        {!token.revoked_at && <Button size="xs" variant="outline" onClick={() => void revoke(token.id)}>{t.mcpRevoke}</Button>}
      </Flex>
    ))}
    {!tokens.length && <Text fontSize="sm" color="var(--text-dim)" mt="2">{t.mcpEmpty}</Text>}
  </>;
}

function Snippet({ label, text, copied, onCopy, copyLabel, copiedLabel }: { label: string; text: string; copied: boolean; onCopy: () => void; copyLabel: string; copiedLabel: string }) {
  return (
    <Box mb="3">
      <Flex justify="space-between" align="center" mb="1">
        <Text fontSize="xs" color="var(--text-dim)" mt="0">{label}</Text>
        <Button size="xs" variant="ghost" onClick={onCopy}>{copied ? copiedLabel : copyLabel}</Button>
      </Flex>
      <Box as="pre" className="mcp-snippet">{text}</Box>
    </Box>
  );
}
