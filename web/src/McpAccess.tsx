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
import "./intake.css";

type TokenRecord = {
  id: string;
  label: string;
  created_at: number;
  revoked_at?: number | null;
};

type Issued = { id: string; label: string; token: string };

const control = { color: "var(--text)", bg: "var(--bg-raised)", borderColor: "var(--border-strong)" };

function installSnippets(token: string) {
  const url = `${window.location.origin}/mcp`;
  const claudeCode = JSON.stringify(
    { mcpServers: { willdeep: { type: "http", url, headers: { Authorization: `Bearer ${token}` } } } },
    null,
    2,
  );
  const chatGpt = `${url}\nAuthorization: Bearer ${token}`;
  return { url, claudeCode, chatGpt };
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

  // 对话框每打开一次刷新一遍；整页挂载时 active 恒为 true，只拉一次。
  useEffect(() => {
    if (!active) return;
    let cancelled = false;
    void fetch("/api/mcp/tokens")
      .then((response) => response.json())
      .then((value: TokenRecord[]) => { if (!cancelled) setTokens(value); })
      .catch((reason: unknown) => { if (!cancelled) setError(String(reason)); });
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

  const snippets = issued ? installSnippets(issued.token) : null;
  return <>
    {error && <Text role="alert" color="var(--danger-text)" mb="3">{error}</Text>}
    <Flex gap="2" mb="4">
      <Input value={label} placeholder={t.mcpLabelPlaceholder} onChange={(event) => setLabel(event.target.value)} {...control} />
      <Button onClick={() => void createToken()}>{t.mcpCreate}</Button>
    </Flex>
    {snippets && (
      <Box className="mcp-issued" mb="4">
        <Text fontSize="sm" color="var(--danger-text)" mb="2">{t.mcpShownOnce}</Text>
        <Text fontSize="xs" color="var(--text-dim)" mb="1">{t.mcpEndpoint}</Text>
        <Text fontSize="sm" mb="3">{snippets.url}</Text>
        <Snippet label={t.mcpClaudeCode} text={snippets.claudeCode} copied={copied === "claude"} onCopy={() => void copy(snippets.claudeCode, "claude")} copyLabel={t.mcpCopy} copiedLabel={t.mcpCopied} />
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
