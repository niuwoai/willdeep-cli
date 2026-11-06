// @vitest-environment jsdom
import { ChakraProvider, defaultSystem } from "@chakra-ui/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { App } from "./App";
import { messages } from "./i18n";
import type { WorkspaceAccess } from "./accessModes";
import type { WorkspaceSummary } from "./workspaceApprovalApi";

// 保留 App 的导航、目录引导、请求与会话状态，只省略无关的侧栏与设置面板。
vi.mock("./RuntimeSidebar", () => ({ RuntimeSidebar: () => null }));
vi.mock("./WorkflowSidebar", () => ({ WorkflowSidebar: () => null }));
vi.mock("./ChildSessions", () => ({ ChildSessions: () => null, ChildSessionOrigin: () => null }));
vi.mock("./SessionCollaboration", () => ({ SessionCollaboration: () => null }));
vi.mock("./ConversationSchedules", () => ({ ConversationSchedules: () => null }));
vi.mock("./RuntimeQueue", () => ({ RuntimeQueue: () => null, IndependentTasks: () => null }));
vi.mock("./ModelPicker", () => ({ ModelPicker: () => null }));
vi.mock("./SharePanel", () => ({ SharePanel: () => null }));
vi.mock("./QuickSettings", () => ({ QuickSettings: () => null }));
vi.mock("./SidebarSettings", () => ({ SidebarSettings: () => null }));
vi.mock("./PermissionsPanel", () => ({ PermissionsPanel: (props: { override: WorkspaceAccess | null; workspaceAccess: WorkspaceAccess; sessionId: string | null }) => <output data-testid="approval-state">{JSON.stringify(props)}</output> }));

const first: WorkspaceSummary = { id: "a", path: "/first", name: "First project", active: true, access: "smart", approval_mode_configured: true };
const other: WorkspaceSummary = { ...first, id: "b", path: "/other", name: "Other project", active: false, approval_mode_configured: false };
const restored = { id: "restored", title: "Remembered conversation", workspace: "/other", created_at: 1, updated_at: 1, pinned_at: null, archived: false, active: false, active_turn_id: null };
const t = messages["zh-CN"];

beforeEach(() => {
  localStorage.clear(); localStorage.setItem("willdeep.language", "zh-CN");
  window.history.replaceState(null, "", "/");
  HTMLElement.prototype.scrollIntoView = vi.fn();
  vi.stubGlobal("ResizeObserver", class { observe() {} unobserve() {} disconnect() {} });
});
afterEach(() => { cleanup(); vi.restoreAllMocks(); vi.unstubAllGlobals(); });

function mount(options: { workspaces?: WorkspaceSummary[]; sessions?: typeof restored[]; override?: WorkspaceAccess; saveFailure?: boolean } = {}) {
  let workspaces = options.workspaces ?? [first, other];
  const posts: { url: string; body: Record<string, unknown> }[] = [];
  const requests: string[] = [];
  vi.stubGlobal("fetch", vi.fn(async (url: string, init?: RequestInit) => {
    requests.push(url);
    if (init?.method === "POST") {
      const body = JSON.parse(String(init.body)) as Record<string, unknown>;
      posts.push({ url, body });
      if (url === "/api/workspaces") {
        workspaces = [...workspaces, other];
        return Response.json(other);
      }
      if (url === "/api/workspaces/approval-mode") {
        if (options.saveFailure) return Response.json({ error: "disk unavailable" }, { status: 500 });
        const updated = { ...workspaces.find((item) => item.path === body.path)!, access: body.approval_mode as WorkspaceAccess, approval_mode_configured: true };
        workspaces = workspaces.map((item) => item.path === updated.path ? updated : item);
        return Response.json(updated);
      }
      if (url === "/api/chat/stream") return Response.json({ error: "test ends at request" }, { status: 400 });
      return Response.json({});
    }
    if (url === "/api/workspaces") return Response.json(workspaces);
    if (url === "/api/sessions") return Response.json(options.sessions ?? []);
    if (url === "/api/sessions/restored") return Response.json({ ...restored, messages: [], approval_mode: options.override ?? "strict", workspace_access: workspaces.find((item) => item.path === "/other")?.access ?? "smart" });
    if (url.startsWith("/api/composer?")) return Response.json({ commands: [], skills: [] });
    if (url.startsWith("/api/runtime/activity?")) return Response.json({ tools: [], artifacts: [], agents: [], tasks: [], gates: [], attention_count: 0 });
    if (url.startsWith("/api/runtime/events?")) return Response.json([]);
    if (url.startsWith("/api/plugins")) return Response.json({ plugins: [], failures: [] });
    if (url === "/api/setup") return Response.json({ ready: true });
    if (url === "/health") return Response.json({ version: "test" });
    return Response.json({}, { status: 404 });
  }));
  const client = new QueryClient({ defaultOptions: { queries: { retry: false, staleTime: Infinity }, mutations: { retry: false } } });
  const view = render(<QueryClientProvider client={client}><ChakraProvider value={defaultSystem}><App /></ChakraProvider></QueryClientProvider>);
  return { ...view, posts, requests };
}

function state() { return JSON.parse(screen.getByTestId("approval-state").textContent ?? "{}") as { override: WorkspaceAccess | null; workspaceAccess: WorkspaceAccess; sessionId: string | null }; }
async function ready(path = "/first") { await waitFor(() => expect(document.querySelector(".project-head.current .project-toggle")?.getAttribute("title")).toBe(path)); }
function newIn(name: string) {
  const group = screen.getByText(name).closest(".project-group")!;
  fireEvent.click(within(group as HTMLElement).getByRole("button", { name: t.newSessionIn, hidden: true }));
}

it.each(["smart", "strict", "full_access"] as const)("gates the initial directory and saves %s as its workspace default", async (mode) => {
  const { posts, requests, container } = mount({ workspaces: [{ ...first, approval_mode_configured: false }] });
  await screen.findByRole("dialog");
  expect(requests.some((url) => url.startsWith("/api/composer?"))).toBe(false);
  if (mode === "strict") fireEvent.click(screen.getByRole("radio", { name: /^严格模式/ }));
  if (mode === "full_access") {
    fireEvent.click(screen.getByRole("radio", { name: /^完全访问/ }));
    fireEvent.click(screen.getByRole("checkbox", { name: /我知道/ }));
  }
  fireEvent.click(screen.getByRole("button", { name: "保存并打开" }));
  await ready();
  await waitFor(() => expect(state().workspaceAccess).toBe(mode));
  expect(state().override).toBeNull();
  fireEvent.change(container.querySelector("textarea")!, { target: { value: "Start work" } });
  fireEvent.click(screen.getByRole("button", { name: t.send }));
  await waitFor(() => expect(posts.some((item) => item.url === "/api/chat/stream")).toBe(true));
  expect(posts.find((item) => item.url === "/api/chat/stream")?.body).toMatchObject({ workspace: "/first", approval_mode: null, approval_confirm: false });
  expect(posts.find((item) => item.url === "/api/workspaces/approval-mode")?.body.confirm).toBe(mode === "full_access");
});

it.each(["smart", "read_only"] as const)("follows an existing %s workspace without submitting a settings write", async (access) => {
  const { posts, container } = mount({ workspaces: [{ ...first, access, approval_mode_configured: access !== "read_only" }] });
  await ready();
  expect(screen.queryByRole("dialog")).toBeNull();
  expect(state().workspaceAccess).toBe(access);
  expect(state().override).toBeNull();
  fireEvent.change(container.querySelector("textarea")!, { target: { value: "Read this project" } });
  fireEvent.click(screen.getByRole("button", { name: t.send }));
  await waitFor(() => expect(posts.some((item) => item.url === "/api/chat/stream")).toBe(true));
  expect(posts.find((item) => item.url === "/api/chat/stream")?.body.approval_mode).toBeNull();
});

it("canceling a new directory preserves the current directory and composer text", async () => {
  const { posts, container } = mount(); await ready();
  const composer = container.querySelector("textarea")!;
  fireEvent.change(composer, { target: { value: "Keep my draft" } });
  newIn("Other project");
  await screen.findByRole("dialog");
  fireEvent.click(screen.getByRole("radio", { name: /^严格模式/ }));
  fireEvent.click(screen.getByRole("button", { name: t.cancel }));
  await ready();
  expect(composer.value).toBe("Keep my draft");
  expect(state().override).toBeNull();
  expect(state().workspaceAccess).toBe("smart");
  expect(posts).toEqual([]);
});

it("adding a directory waits for the same guide and cancellation preserves both drafts", async () => {
  const { posts, container } = mount({ workspaces: [first] }); await ready();
  fireEvent.change(container.querySelector("textarea")!, { target: { value: "Keep conversation draft" } });
  fireEvent.click(screen.getByRole("button", { name: t.addWorkspace, hidden: true }));
  const path = screen.getByRole("textbox", { name: t.addWorkspace, hidden: true }) as HTMLInputElement;
  fireEvent.change(path, { target: { value: "/other" } });
  fireEvent.keyDown(path, { key: "Enter" });
  await screen.findByRole("dialog");
  fireEvent.click(screen.getByRole("button", { name: t.cancel }));
  await ready();
  expect(path.value).toBe("/other");
  expect(container.querySelector("textarea")?.value).toBe("Keep conversation draft");
  expect(posts.map((item) => item.url)).toEqual(["/api/workspaces"]);
});

it("keeps a failed selection open without replacing the original directory", async () => {
  const { container } = mount({ saveFailure: true }); await ready();
  fireEvent.change(container.querySelector("textarea")!, { target: { value: "Unsent text" } });
  newIn("Other project"); await screen.findByRole("dialog");
  fireEvent.click(screen.getByRole("button", { name: "保存并打开" }));
  expect((await screen.findByRole("alert")).textContent).toContain("disk unavailable");
  expect(container.querySelector(".project-head.current .project-toggle")?.getAttribute("title")).toBe("/first");
  expect(container.querySelector("textarea")?.value).toBe("Unsent text");
});

it.each(["deep-link", "remembered"])("restores the existing session override after the %s guide", async (entry) => {
  if (entry === "deep-link") window.history.replaceState(null, "", "/?session=restored");
  else localStorage.setItem("willdeep.web.last-session./other", "restored");
  const { posts } = mount({ workspaces: entry === "deep-link" ? [first, other] : [other], sessions: [restored], override: "strict" });
  const dialog = await screen.findByRole("dialog");
  expect(within(dialog).getByText("/other")).toBeTruthy();
  fireEvent.click(screen.getByRole("radio", { name: /^完全访问/ }));
  fireEvent.click(screen.getByRole("checkbox", { name: /我知道/ }));
  fireEvent.click(screen.getByRole("button", { name: "保存并打开" }));
  await ready("/other");
  await waitFor(() => expect(state().sessionId).toBe("restored"));
  expect(state().override).toBe("strict");
  expect(state().workspaceAccess).toBe("full_access");
  expect(posts.map((item) => item.url)).toEqual(["/api/workspaces/approval-mode"]);
});

it("cross-directory session navigation guides first and preserves the session override", async () => {
  const { posts, container } = mount({ sessions: [restored], override: "workspace_write" }); await ready();
  fireEvent.click(container.querySelector('.project-toggle[title="/other"]')!);
  fireEvent.click(await screen.findByRole("button", { name: "Remembered conversation", hidden: true }));
  await screen.findByRole("dialog");
  fireEvent.click(screen.getByRole("button", { name: "保存并打开" }));
  await ready("/other");
  await waitFor(() => expect(state().sessionId).toBe("restored"));
  expect(state().override).toBe("workspace_write");
  expect(posts.map((item) => item.url)).toEqual(["/api/workspaces/approval-mode"]);
});
