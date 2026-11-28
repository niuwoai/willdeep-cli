// @vitest-environment jsdom
import { afterEach, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { CliInstall, type CliInstallPlan } from "./CliInstall";
import { messages } from "./i18n";

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

function renderPage() {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } });
  return render(<QueryClientProvider client={client}><CliInstall messages={messages["zh-CN"]} /></QueryClientProvider>);
}

function planOf(plan: CliInstallPlan) {
  return vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
    const url = String(input);
    if (url.endsWith("/api/desktop/cli") && (!init || !init.method || init.method === "GET")) {
      return new Response(JSON.stringify(plan), { status: 200 });
    }
    if (url.endsWith("/api/desktop/cli/install")) {
      const body = JSON.parse(String(init?.body)) as { manager: string };
      const chosen = plan.managers.find((manager) => manager.id === body.manager);
      return new Response(JSON.stringify({ command: chosen?.command ?? "", exit_code: 0, output: "done" }), { status: 200 });
    }
    return new Response("", { status: 404 });
  });
}

it("浏览器里不拉取方案，只说明安装只能在桌面版进行", () => {
  vi.stubGlobal("__willdeepDesktop", undefined);
  const fetchMock = vi.fn();
  vi.stubGlobal("fetch", fetchMock);
  renderPage();
  expect(screen.getByRole("alert").textContent).toBe(messages["zh-CN"].cliDesktopOnly);
  expect(fetchMock).not.toHaveBeenCalled();
});

it("桌面版列出可执行的安装方式，点安装只提交管理器 id", async () => {
  vi.stubGlobal("__willdeepDesktop", true);
  const fetchMock = planOf({
    os: "macos",
    managers: [{ id: "brew", command: "brew install willdeep-cli" }, { id: "npm", command: "npm install -g @niuwoai/willdeep" }],
    manual: [],
  });
  vi.stubGlobal("fetch", fetchMock);
  renderPage();
  expect(await screen.findByText("brew install willdeep-cli")).toBeTruthy();
  expect(screen.getByText("npm install -g @niuwoai/willdeep")).toBeTruthy();
  const buttons = screen.getAllByRole("button", { name: messages["zh-CN"].cliInstall });
  fireEvent.click(buttons[0]);
  expect(await screen.findByRole("status")).toBeTruthy();
  const posted = fetchMock.mock.calls.find((call) => String(call[0]).endsWith("/install"));
  expect(JSON.parse(String(posted?.[1].body))).toEqual({ manager: "brew" });
});

it("Windows 只展示命令，没有安装按钮", async () => {
  vi.stubGlobal("__willdeepDesktop", true);
  vi.stubGlobal("fetch", planOf({
    os: "windows",
    managers: [],
    manual: [{ label: "npm", command: "npm install -g @niuwoai/willdeep" }],
  }));
  renderPage();
  expect(await screen.findByText(messages["zh-CN"].cliManualHint)).toBeTruthy();
  expect(screen.getByText("npm install -g @niuwoai/willdeep")).toBeTruthy();
  expect(screen.queryByRole("button", { name: messages["zh-CN"].cliInstall })).toBeNull();
});

it("这台电脑没有任何可用的安装方式时给出说明", async () => {
  vi.stubGlobal("__willdeepDesktop", true);
  vi.stubGlobal("fetch", planOf({ os: "macos", managers: [], manual: [] }));
  renderPage();
  expect(await screen.findByText(messages["zh-CN"].cliNone)).toBeTruthy();
});
