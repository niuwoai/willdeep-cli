// @vitest-environment jsdom
import { afterEach, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { McpServers, draftToRequest, emptyDraft } from "./McpServers";
import { messages } from "./i18n";

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

const text = messages["zh-CN"];

function renderPage() {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } });
  return render(<QueryClientProvider client={client}><McpServers messages={text} /></QueryClientProvider>);
}

it("列出服务时不出现 token 明文，并说明环境变量的时机", async () => {
  vi.stubGlobal("fetch", vi.fn(async () => new Response(JSON.stringify({
    servers: [{
      name: "docs", transport: "http", command: null, args: [], env: [], url: "https://docs.example.com/mcp",
      headers: [], auth: "token", token_set: true, bearer_token_env: null, oauth_client_id: null,
      startup_timeout_seconds: 30, enabled: true,
    }],
    skipped: [],
  }), { status: 200 })));
  renderPage();
  expect(await screen.findByText("docs")).toBeTruthy();
  expect(screen.getByText(text.mcpServersEnvNote)).toBeTruthy();
  expect(document.body.textContent).not.toContain("super-secret");
});

it("添加时只按所选传输和认证提交字段", async () => {
  const fetchMock = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
    if (init?.method === "POST") return new Response("{}", { status: 200 });
    return new Response(JSON.stringify({ servers: [], skipped: [] }), { status: 200 });
  });
  vi.stubGlobal("fetch", fetchMock);
  renderPage();
  fireEvent.click(await screen.findByRole("button", { name: text.mcpServersAdd }));
  fireEvent.change(screen.getByLabelText(text.mcpServersName), { target: { value: "docs" } });
  fireEvent.click(screen.getByLabelText(text.mcpServersHttp));
  fireEvent.change(screen.getByLabelText(text.mcpServersUrl), { target: { value: "https://docs.example.com/mcp" } });
  fireEvent.click(screen.getByLabelText(text.mcpServersAuthToken));
  fireEvent.change(screen.getByLabelText(text.mcpServersToken), { target: { value: "super-secret-value" } });
  fireEvent.click(screen.getByRole("button", { name: text.mcpServersSave }));
  await screen.findByRole("button", { name: text.mcpServersAdd });
  const posted = fetchMock.mock.calls.find((call) => call[1]?.method === "POST");
  const body = JSON.parse(String(posted?.[1].body)) as Record<string, unknown>;
  expect(body).toEqual({ name: "docs", url: "https://docs.example.com/mcp", headers: {}, token: "super-secret-value" });
  expect(body).not.toHaveProperty("command");
  expect(body).not.toHaveProperty("bearer_token_env");
});

it("本地命令的参数和变量按行拆开", () => {
  const body = draftToRequest({
    ...emptyDraft(),
    name: "local", transport: "stdio", command: "serve", args: "--port 9", env: "TOKEN=secret\nPLAIN=1",
  });
  expect(body).toMatchObject({ command: "serve", args: ["--port", "9"], env: { TOKEN: "secret", PLAIN: "1" } });
  expect(body).not.toHaveProperty("url");
  expect(body).not.toHaveProperty("token");
});
