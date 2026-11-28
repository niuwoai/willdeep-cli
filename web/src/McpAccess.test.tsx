// @vitest-environment jsdom
import type { ReactNode } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { ChakraProvider, defaultSystem } from "@chakra-ui/react";
import { describe, expect, it } from "vitest";
import { McpAccess, installSnippets, mcpEndpoint } from "./McpAccess";
import type { IntakeView } from "./intakeApi";
import { messages } from "./i18n";

function render(node: ReactNode) {
  return renderToStaticMarkup(<ChakraProvider value={defaultSystem}>{node}</ChakraProvider>);
}

function intake(overrides: Partial<IntakeView>): IntakeView {
  return {
    enabled: true,
    listen: "127.0.0.1:51713",
    public_url: "",
    running: false,
    runtime_managed: true,
    writable: true,
    sites: [],
    forms: [],
    webhooks: [],
    pending: [],
    bindings: [],
    ...overrides,
  };
}

describe("McpAccess", () => {
  it.each(["zh-CN", "en", "ja"] as const)("%s 打开时说明用途并给出创建入口", (language) => {
    const html = render(
      <McpAccess messages={messages[language]} open onOpenChange={() => undefined} />,
    );
    expect(html).toContain(messages[language].mcpTitle);
    // 英文说明里的撇号会被转义成实体，取撇号之前的部分断言。
    expect(html).toContain(messages[language].mcpIntro.split("'")[0]);
    expect(html).toContain(messages[language].mcpCreate);
    // 还没创建 token 时不出现安装指令，避免复制到一个空的。
    expect(html).not.toContain("Bearer");
  });

  it("公网入口开着且有对外域名时，安装片段用公网地址", () => {
    const endpoint = mcpEndpoint(intake({ running: true, public_url: "https://wd.example.com" }));
    expect(endpoint).toEqual({ url: "https://wd.example.com/mcp", public: true });
    const snippets = installSnippets("secret-token", endpoint.url);
    expect(snippets.willdeep).toContain('url = "https://wd.example.com/mcp"');
    expect(snippets.willdeep).toContain('token = "secret-token"');
    expect(snippets.codex).toContain("bearer_token_env_var = \"WILLDEEP_MCP_TOKEN\"");
    expect(snippets.claudeCode).toContain("https://wd.example.com/mcp");
  });

  it("公网地址是回环或入口没开时，退回本机地址并标成内部", () => {
    expect(mcpEndpoint(intake({ running: true, public_url: "http://127.0.0.1:51713" })).public).toBe(false);
    expect(mcpEndpoint(intake({ running: false, public_url: "https://wd.example.com" })).public).toBe(false);
    expect(mcpEndpoint(null).public).toBe(false);
  });

  it("关闭时不渲染内容", () => {
    const html = render(
      <McpAccess messages={messages["zh-CN"]} open={false} onOpenChange={() => undefined} />,
    );
    expect(html).not.toContain(messages["zh-CN"].mcpIntro);
  });
});
