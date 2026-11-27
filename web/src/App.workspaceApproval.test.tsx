// @vitest-environment jsdom
import { ChakraProvider, defaultSystem } from "@chakra-ui/react";
import { focusManager, QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import type { ConversationItem } from "./ConversationCard";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { App } from "./App";
import { messages } from "./i18n";
import type { WorkspaceAccess } from "./accessModes";
import type { WorkspaceSummary } from "./workspaceApprovalApi";
import type { RuntimeActivity, RuntimeGate } from "./RuntimeSidebar";
import { queueText } from "./queueMessages";
import { appendSelectionQuote } from "./useChatSelection";
import type { Reference } from "./useComposerReferences";
import type { PluginView } from "./plugins";

// 保留 App 的导航、目录引导、请求与会话状态，只省略无关的侧栏与设置面板。
const desktop = vi.hoisted(() => ({ enabled: false }));
vi.mock("./desktopHost", async (importOriginal) => ({
  ...await importOriginal<typeof import("./desktopHost")>(),
  get isDesktop() { return desktop.enabled; },
}));
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
const restored = { id: "restored", title: "Remembered conversation", workspace: "/other", created_at: 1, updated_at: 1, pinned_at: null as number | null, archived: false, active: false, active_turn_id: null };
const t = messages["zh-CN"];

beforeEach(() => {
  localStorage.clear(); localStorage.setItem("willdeep.language", "zh-CN");
  window.history.replaceState(null, "", "/");
  HTMLElement.prototype.scrollIntoView = vi.fn();
  vi.stubGlobal("ResizeObserver", class { observe() {} unobserve() {} disconnect() {} });
});
afterEach(() => { cleanup(); desktop.enabled = false; vi.restoreAllMocks(); vi.unstubAllGlobals(); });

function mount(options: { workspaces?: WorkspaceSummary[]; sessions?: (typeof restored & { parent_session_id?: string })[]; history?: ConversationItem[]; override?: WorkspaceAccess; saveFailure?: boolean; activity?: RuntimeActivity; queueFailures?: number; chatResponse?: Promise<Response> | ((signal: AbortSignal) => Response); forkFailures?: number; sessionActionFailure?: boolean; references?: Reference[]; plugins?: PluginView[] } = {}) {
  let workspaces = options.workspaces ?? [first, other];
  let sessions = options.sessions ?? [];
  let queueFailures = options.queueFailures ?? 0;
  let forkFailures = options.forkFailures ?? 0;
  let fork: typeof restored | undefined;
  const posts: { url: string; body: Record<string, unknown> }[] = [];
  const deletes: { url: string; body: Record<string, unknown> }[] = [];
  const requests: string[] = [];
  vi.stubGlobal("fetch", vi.fn(async (url: string, init?: RequestInit) => {
    requests.push(url);
    if (init?.method === "DELETE") {
      const body = JSON.parse(String(init.body)) as Record<string, unknown>;
      deletes.push({ url, body });
      sessions = sessions.filter(({ id }) => url !== `/api/sessions/${id}`);
      return new Response(null, { status: 204 });
    }
    if (init?.method === "POST") {
      const body = JSON.parse(String(init.body ?? "{}")) as Record<string, unknown>;
      if (/^\/api\/plugins\/[^/]+\/host\/chatSend$/.test(url)) return Response.json({ text: body.text });
      const sessionAction = /^\/api\/sessions\/([^/]+)\/(rename|pin|unpin|archive|unarchive)$/.exec(url);
      if (sessionAction) {
        posts.push({ url, body });
        if (options.sessionActionFailure) return new Response("session store unavailable", { status: 503 });
        const [, id, action] = sessionAction;
        sessions = sessions.map((item) => {
          if (item.id !== id) return item;
          switch (action) {
            case "rename": return { ...item, title: String(body.title) };
            case "pin": return { ...item, pinned_at: 2 };
            case "unpin": return { ...item, pinned_at: null };
            default: return { ...item, archived: action === "archive" };
          }
        });
        return new Response(null, { status: 204 });
      }
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
      if (url === "/api/sessions/restored/fork") {
        if (forkFailures-- > 0) return Response.json({ error: "fork unavailable" }, { status: 503 });
        fork = { ...restored, id: "forked", title: typeof body.title === "string" ? body.title : `${restored.title} (fork)` };
        return Response.json({ id: fork.id }, { status: 201 });
      }
      if (url === "/api/sessions/restored/queue" && queueFailures-- > 0) return Response.json({ error: "queue unavailable" }, { status: 503 });
      if (url === "/api/chat/stream") {
        if (typeof options.chatResponse === "function" && init.signal) return options.chatResponse(init.signal);
        return options.chatResponse instanceof Promise ? options.chatResponse : Response.json({ error: "test ends at request" }, { status: 400 });
      }
      return Response.json({});
    }
    if (url === "/api/workspaces") return Response.json(workspaces);
    if (url === "/api/sessions") return Response.json([...sessions, ...(fork ? [fork] : [])]);
    if (fork && url === `/api/sessions/${fork.id}`) return Response.json({ ...fork, messages: [] });
    if (url === "/api/sessions/restored") return Response.json({ ...restored, messages: options.history ?? [], approval_mode: options.override ?? "strict", workspace_access: workspaces.find((item) => item.path === "/other")?.access ?? "smart" });
    const detail = sessions.find((item) => url === `/api/sessions/${item.id}`);
    if (detail) return Response.json({ ...detail, messages: [], workspace_access: "smart" });
    if (url.startsWith("/api/composer?")) return Response.json({ commands: [], skills: [], references: options.references ?? [] });
    if (url.startsWith("/api/runtime/activity?")) return Response.json(options.activity ?? { tools: [], artifacts: [], agents: [], tasks: [], gates: [], attention_count: 0 });
    if (url.startsWith("/api/runtime/events?")) return Response.json([]);
    if (url.startsWith("/api/plugins")) return Response.json({ plugins: options.plugins ?? [], failures: [] });
    if (url === "/api/setup") return Response.json({ ready: true });
    if (url === "/health") return Response.json({ version: "test" });
    return Response.json({}, { status: 404 });
  }));
  const client = new QueryClient({ defaultOptions: { queries: { retry: false, staleTime: Infinity }, mutations: { retry: false } } });
  const view = render(<QueryClientProvider client={client}><ChakraProvider value={defaultSystem}><App /></ChakraProvider></QueryClientProvider>);
  return { ...view, posts, deletes, requests };
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

async function mountForkSource(forkFailures = 0, draft = "") {
  const view = mount({ workspaces: [{ ...other, active: true, approval_mode_configured: true }], sessions: [restored], forkFailures });
  await ready("/other");
  fireEvent.click(await screen.findByRole("button", { name: restored.title, hidden: true }, { timeout: 5000 }));
  await waitFor(() => expect(state().sessionId).toBe(restored.id), { timeout: 5000 });
  if (draft) fireEvent.change(view.container.querySelector("textarea")!, { target: { value: draft } });
  const open = () => fireEvent.click(screen.getByRole("button", { name: t.forkSession, hidden: true }));
  open();
  const dialog = await screen.findByRole("dialog", { name: t.forkDialogTitle });
  return { ...view, dialog, open };
}

it("cancels a session fork without creating one and resets the title on reopen", async () => {
  const { dialog, open, posts } = await mountForkSource();
  expect(within(dialog).getByText(restored.title)).toBeTruthy();
  fireEvent.change(within(dialog).getByRole("textbox", { name: t.forkTitleLabel }), { target: { value: "Discard this title" } });
  fireEvent.click(within(dialog).getByRole("button", { name: t.cancel }));
  await waitFor(() => expect(screen.queryByRole("dialog", { name: t.forkDialogTitle })).toBeNull());
  expect(posts).toEqual([]);
  expect(state().sessionId).toBe(restored.id);
  open();
  const input = await screen.findByRole<HTMLInputElement>("textbox", { name: t.forkTitleLabel });
  expect(input.value).toBe("");
});

it.each([
  { input: "  Follow-up  ", body: { title: "Follow-up" }, name: "Follow-up" },
  { input: "   ", body: {}, name: `${restored.title} (fork)` },
])("creates and opens the session fork in the same window with title $name", async ({ input, body, name }) => {
  const openWindow = vi.spyOn(window, "open").mockReturnValue(null);
  const { dialog, posts } = await mountForkSource(0, "Parent draft");
  fireEvent.change(within(dialog).getByRole("textbox", { name: t.forkTitleLabel }), { target: { value: input } });
  fireEvent.click(within(dialog).getByRole("button", { name: t.forkConfirm }));
  await waitFor(() => expect(state().sessionId).toBe("forked"));
  expect(posts).toEqual([{ url: "/api/sessions/restored/fork", body }]);
  expect(screen.getByRole("button", { name, hidden: true })).toBeTruthy();
  expect(openWindow).not.toHaveBeenCalled();
});

it("keeps the title in the dialog when creating a session fork fails", async () => {
  const { dialog, posts } = await mountForkSource(1);
  const input = within(dialog).getByRole<HTMLInputElement>("textbox", { name: t.forkTitleLabel });
  fireEvent.change(input, { target: { value: "Retry title" } });
  fireEvent.click(within(dialog).getByRole("button", { name: t.forkConfirm }));
  expect((await within(dialog).findByRole("alert")).textContent).toContain("fork unavailable");
  expect(input.value).toBe("Retry title");
  expect(state().sessionId).toBe(restored.id);
  fireEvent.click(within(dialog).getByRole("button", { name: t.forkConfirm }));
  await waitFor(() => expect(state().sessionId).toBe("forked"));
  expect(posts).toHaveLength(2);
});

function rowActions(title: string) {
  return screen.getByRole("button", { name: t.sessionActionsFor.replace("{title}", title), hidden: true });
}

async function openRowActions(title: string) {
  const trigger = rowActions(title);
  fireEvent.click(trigger);
  // jsdom gives the clipping ancestor a zero-sized rectangle. Resolve this trigger's own portal.
  const menu = within(await waitFor(() => {
    const content = document.getElementById(trigger.getAttribute("aria-controls") ?? "");
    if (!content || content.hidden) throw new Error(`Menu did not open for ${title}`);
    return content;
  }));
  return (name: string) => menu.getByText(name).closest('[role="menuitem"]')!;
}

it("routes actions to a nested row and requires confirmation to delete it", async () => {
  const child = { ...restored, id: "child", title: "Nested conversation", workspace: "/first", parent_session_id: "root" };
  const root = { ...child, id: "root", title: "Root conversation", parent_session_id: undefined };
  const prompt = vi.spyOn(window, "prompt").mockReturnValue("  Renamed child  ");
  const { posts, deletes, requests } = mount({ workspaces: [first], sessions: [root, child] });
  await ready();
  await screen.findByRole("button", { name: t.sessionActionsFor.replace("{title}", child.title), hidden: true });
  expect(rowActions(child.title).closest(".session-row")?.classList.contains("nested")).toBe(true);

  const actions = await openRowActions(child.title);
  const fork = actions(t.forkSession);
  expect(fork.getAttribute("aria-disabled")).toBe("true");
  fireEvent.click(fork);
  expect(posts).toEqual([]);
  fireEvent.click(actions(t.renameSession));
  expect(prompt).toHaveBeenCalledWith(t.renamePrompt, child.title);
  const renamed = "Renamed child";
  await screen.findByRole("button", { name: t.sessionActionsFor.replace("{title}", renamed), hidden: true });

  fireEvent.click((await openRowActions(renamed))(t.pinSession));
  await waitFor(() => expect(rowActions(renamed).closest(".session-row")?.querySelector(`[aria-label="${t.pinned}"]`)).toBeTruthy());
  fireEvent.click((await openRowActions(renamed))(t.unpinSession));
  await waitFor(() => expect(rowActions(renamed).closest(".session-row")?.querySelector(`[aria-label="${t.pinned}"]`)).toBeNull());

  fireEvent.click((await openRowActions(renamed))(t.archiveSession));
  fireEvent.click((await screen.findByText(`${t.showArchived} (1)`)).closest("button")!);
  expect(rowActions(renamed).closest(".archived-sessions")).toBeTruthy();
  fireEvent.click((await openRowActions(renamed))(t.unarchiveSession));
  await waitFor(() => expect(rowActions(renamed).closest(".archived-sessions")).toBeNull());

  fireEvent.click((await openRowActions(renamed))(t.deleteSession));
  const dialog = within(await screen.findByRole("alertdialog", { name: t.deleteDialogTitle }));
  expect(dialog.getByText(renamed)).toBeTruthy();
  expect(deletes).toEqual([]);
  fireEvent.click(dialog.getByRole("button", { name: t.cancel }));
  await waitFor(() => expect(screen.queryByRole("alertdialog")).toBeNull());
  fireEvent.click((await openRowActions(renamed))(t.deleteSession));
  fireEvent.click(within(await screen.findByRole("alertdialog")).getByRole("button", { name: t.confirmDelete }));
  await waitFor(() => expect(screen.queryByRole("button", { name: t.sessionActionsFor.replace("{title}", renamed), hidden: true })).toBeNull());
  expect(deletes).toEqual([{ url: "/api/sessions/child", body: { confirmation: "child" } }]);
  expect(posts).toEqual([
    { url: "/api/sessions/child/rename", body: { title: renamed } },
    ...["pin", "unpin", "archive", "unarchive"].map((action) => ({ url: `/api/sessions/child/${action}`, body: {} })),
  ]);
  expect(requests.filter((url) => url === "/api/sessions/child")).toHaveLength(1);
});

it("forks the clicked row while a different conversation is selected", async () => {
  const selected = { ...restored, id: "selected", title: "Selected conversation" };
  const { posts } = mount({ workspaces: [{ ...other, approval_mode_configured: true }], sessions: [selected, restored] });
  await ready("/other");
  fireEvent.click(await screen.findByRole("button", { name: selected.title, hidden: true }));
  await waitFor(() => expect(state().sessionId).toBe(selected.id));
  fireEvent.click((await openRowActions(restored.title))(t.forkSession));
  const dialog = within(await screen.findByRole("dialog", { name: t.forkDialogTitle }));
  expect(dialog.getByText(restored.title)).toBeTruthy();
  expect(state().sessionId).toBe(selected.id);
  fireEvent.click(dialog.getByRole("button", { name: t.forkConfirm }));
  await waitFor(() => expect(state().sessionId).toBe("forked"));
  expect(posts).toEqual([{ url: "/api/sessions/restored/fork", body: {} }]);
});

it("keeps running-session restrictions separate from failures returned by the API", async () => {
  const active = { ...restored, workspace: "/first", active: true };
  const { posts } = mount({ workspaces: [first], sessions: [active], sessionActionFailure: true });
  await ready();
  const menu = await openRowActions(active.title);
  for (const name of [t.renameSession, t.forkSession, t.archiveSession, t.deleteSession]) {
    const item = menu(name);
    expect(item.getAttribute("aria-disabled")).toBe("true");
    fireEvent.click(item);
  }
  expect(posts).toEqual([]);
  expect(screen.queryByRole("alert")).toBeNull();
  fireEvent.click(menu(t.pinSession));
  expect((await screen.findByRole("alert", { hidden: true })).textContent).toBe(`${t.sessionActionFailed}: session store unavailable`);
  expect(posts).toEqual([{ url: "/api/sessions/restored/pin", body: {} }]);
});

const runningActivity: RuntimeActivity = {
  tools: [{ id: "read", session_id: "restored", turn_id: "turn", task_id: "task", agent_id: "agent", name: "read_file", status: "completed", started_at_ms: 1, completed_at_ms: 2 },
    { id: "list", session_id: "restored", turn_id: "turn", task_id: "task", agent_id: "agent", name: "list_directory", status: "running", started_at_ms: 3, completed_at_ms: null }],
  artifacts: [], agents: [], tasks: [{ id: "task", session_id: "restored", turn_id: "turn", agent_id: "agent", status: "running", profile: null, elapsed_seconds: 3, exit_code: null, failure_domain: null }],
  gates: [], attention_count: 0,
};

async function mountRunning(activity = runningActivity, queueFailures = 0) {
  localStorage.setItem("willdeep.web.last-session./other", "restored");
  const view = mount({ workspaces: [{ ...other, active: true, approval_mode_configured: true }], sessions: [{ ...restored, active: true }], activity, queueFailures });
  await ready("/other");
  await waitFor(() => expect(state().sessionId).toBe("restored"));
  await waitFor(() => expect(view.requests.some((url) => url.includes("/api/runtime/activity?"))).toBe(true));
  return { ...view, composer: view.container.querySelector("textarea")! };
}

it("switches Desktop sessions after submission without opening a window or losing the draft", async () => {
  desktop.enabled = true;
  localStorage.setItem("willdeep.web.last-session./other", restored.id);
  const second = { ...restored, id: "second", title: "Other conversation" };
  const encoder = new TextEncoder();
  let activeSignal: AbortSignal | undefined;
  let stream: ReadableStreamDefaultController<Uint8Array> | undefined;
  const open = vi.spyOn(window, "open").mockReturnValue(null);
  const { container, posts } = mount({
    workspaces: [{ ...other, active: true, approval_mode_configured: true }],
    sessions: [restored, second],
    chatResponse: (signal) => {
      activeSignal = signal;
      return new Response(new ReadableStream<Uint8Array>({ start(controller) {
        stream = controller;
        signal.addEventListener("abort", () => controller.error(signal.reason), { once: true });
      } }));
    },
  });
  await waitFor(() => expect(state().sessionId).toBe(restored.id));
  const composer = container.querySelector("textarea")!;
  fireEvent.change(composer, { target: { value: "Submit a turn" } });
  fireEvent.keyDown(composer, { key: "Enter" });
  await waitFor(() => expect(activeSignal).toBeDefined());

  fireEvent.change(composer, { target: { value: "Keep this new draft" } });
  fireEvent.click(screen.getByRole("button", { name: second.title, hidden: true }));
  expect(activeSignal?.aborted).toBe(false);
  expect(state().sessionId).toBe(restored.id);
  stream?.enqueue(encoder.encode(`data: ${JSON.stringify({ type: "submitted", session_id: restored.id, turn_id: "turn" })}\n\n`));
  await waitFor(() => expect(state().sessionId).toBe(second.id));
  expect(activeSignal?.aborted).toBe(true);
  expect(composer.value).toBe("");

  fireEvent.click(screen.getByRole("button", { name: restored.title, hidden: true }));
  await waitFor(() => expect(state().sessionId).toBe(restored.id));
  expect(composer.value).toBe("Keep this new draft");
  expect(posts.map((post) => post.url)).toEqual(["/api/chat/stream"]);
  expect(open).not.toHaveBeenCalled();
});

const referenceFile: Reference = { kind: "file", name: "src/中文 file.rs", value: '@file:"src/中文 file.rs"' };
const referenceAgent: Reference = { kind: "agent", name: "Reviewer", value: "@agent:reviewer" };

async function selectReferenceAt(composer: HTMLTextAreaElement, text: string, cursor: number, entry: Reference, byKeyboard = false) {
  fireEvent.change(composer, { target: { value: text, selectionStart: cursor, selectionEnd: cursor } });
  const option = await screen.findByRole("option", { name: entry.name });
  if (byKeyboard) fireEvent.keyDown(composer, { key: "Enter" });
  else fireEvent.click(option);
  await screen.findByRole("button", { name: t.removeReference.replace("{name}", entry.name) });
}

it("removes a selected file as one unit with Backspace, then sends the full reference in place", async () => {
  const { posts, container } = mount({ workspaces: [first], references: [referenceFile] });
  await ready();
  const composer = container.querySelector("textarea")!;
  const text = "比较 @src 后文";
  const cursor = text.indexOf(" 后文");
  await selectReferenceAt(composer, text, cursor, referenceFile, true);
  expect(composer.value).toBe("比较  后文");
  composer.setSelectionRange(3, 3);
  expect(fireEvent.keyDown(composer, { key: "Backspace" })).toBe(false);
  expect(screen.queryByRole("button", { name: t.removeReference.replace("{name}", referenceFile.name) })).toBeNull();
  expect(composer.value).toBe("比较  后文");
  await selectReferenceAt(composer, text, cursor, referenceFile);
  fireEvent.click(screen.getByRole("button", { name: t.send }));
  await waitFor(() => expect(posts).toHaveLength(1));
  expect(posts[0].body.prompt).toBe(`比较 ${referenceFile.value} 后文`);
});

it("keeps a reference-only draft through queue failure and clears it when accepted", async () => {
  localStorage.setItem("willdeep.web.last-session./other", restored.id);
  const { container, posts } = mount({ workspaces: [{ ...other, active: true, approval_mode_configured: true }], sessions: [restored], queueFailures: 1, references: [referenceAgent] });
  await waitFor(() => expect(state().sessionId).toBe(restored.id));
  const composer = container.querySelector("textarea")!;
  await selectReferenceAt(composer, "@rev", 4, referenceAgent);
  expect(composer.value).toBe("");
  fireEvent.keyDown(composer, { key: "Enter", ctrlKey: true });
  await screen.findByText(/queue unavailable/);
  expect(screen.getByRole("button", { name: t.removeReference.replace("{name}", referenceAgent.name) })).toBeTruthy();
  fireEvent.keyDown(composer, { key: "Enter", ctrlKey: true });
  await screen.findByText(/已保存到待发队列/);
  expect(posts).toHaveLength(2);
  expect(posts[0].body.prompt).toBe(referenceAgent.value);
  expect(posts[1].body).toEqual(posts[0].body);
  expect(screen.queryByRole("button", { name: t.removeReference.replace("{name}", referenceAgent.name) })).toBeNull();
});

const videoPlugin: PluginView = {
  id: "fixture-video", name: "Video plugin", version: "1.0.0", description: null, source: "fixture", enabled: true,
  approval_gap: null, permissions: [], inferred_permissions: [], mcp_servers: [], destinations: [], settings: [], unsupported: [], file_pickers: [],
  commands: [{ id: "open", title: "Open fixture video studio", icon: null, handler: "host" }], menus: { "composer.more": ["open"] },
};

it("refreshes the desktop plugin menu when returning from another window", async () => {
  desktop.enabled = true;
  const plugins: PluginView[] = [];
  const { requests } = mount({ workspaces: [first], plugins });
  await ready();
  await waitFor(() => expect(requests).toContain("/api/plugins?locale=zh-CN"));
  expect(screen.queryByRole("button", { name: t.pluginComposerMore })).toBeNull();
  try {
    act(() => focusManager.setFocused(false));
    plugins.push(videoPlugin);
    act(() => focusManager.setFocused(true));
    await screen.findByRole("button", { name: t.pluginComposerMore });
  } finally {
    act(() => focusManager.setFocused(undefined));
  }
});

it.each([false, true])("the composer plus opens the filtered file picker with a plugin installed (Desktop: %s)", async (desktopEnabled) => {
  desktop.enabled = desktopEnabled;
  const { posts, container } = mount({ workspaces: [first], plugins: [videoPlugin] });
  await ready();
  const picker = container.querySelector<HTMLInputElement>('input[type="file"]')!;
  const open = vi.spyOn(picker, "click").mockImplementation(() => undefined);
  await screen.findByRole("button", { name: t.pluginComposerMore });
  fireEvent.click(screen.getByRole("button", { name: t.composerAddFiles }));
  expect(open).toHaveBeenCalledOnce();
  expect(picker.accept.split(",")).toEqual(expect.arrayContaining([".png", ".jpg", ".pdf"]));
  expect(picker.multiple).toBe(true);
  expect(screen.queryByText("Open fixture video studio")).toBeNull();
  const oversized = new File([new Uint8Array(700 * 1024 + 1)], "large.pdf", { type: "application/pdf" });
  fireEvent.change(picker, { target: { files: [oversized] } });
  const error = await screen.findByText(/large\.pdf.*700 KiB/);
  expect(error.closest(".composer-shell")).not.toBeNull();

  const content = "Selected from disk";
  const file = new File([content], "notes.txt", { type: "text/plain" });
  Object.defineProperty(file, "arrayBuffer", { value: () => Promise.resolve(new TextEncoder().encode(content).buffer) });
  fireEvent.change(picker, { target: { files: [file] } });
  await screen.findByText("notes.txt");
  fireEvent.click(screen.getByRole("button", { name: t.send }));
  await waitFor(() => expect(posts).toHaveLength(1));
  expect(posts[0].body.attachments).toEqual([{ kind: "text", name: "notes.txt", content }]);
});

it("releases the new session's file picker while the old read is pending", async () => {
  const second = { ...restored, id: "second-draft", title: "Second draft" };
  localStorage.setItem("willdeep.web.last-session./other", restored.id);
  const { container } = mount({ workspaces: [{ ...other, active: true, approval_mode_configured: true }], sessions: [restored, second] });
  await waitFor(() => expect(state().sessionId).toBe(restored.id));
  const picker = container.querySelector<HTMLInputElement>('input[type="file"]')!;
  const content = new TextEncoder().encode("Notes for a conversation").buffer;
  let finishOld!: (bytes: ArrayBuffer) => void;
  let finishNew!: (bytes: ArrayBuffer) => void;
  const oldFile = new File([content], "old.txt", { type: "text/plain" });
  const newFile = new File([content], "new.txt", { type: "text/plain" });
  Object.defineProperty(oldFile, "arrayBuffer", { value: () => new Promise<ArrayBuffer>((resolve) => { finishOld = resolve; }) });
  Object.defineProperty(newFile, "arrayBuffer", { value: () => new Promise<ArrayBuffer>((resolve) => { finishNew = resolve; }) });
  fireEvent.change(picker, { target: { files: [oldFile] } });
  await screen.findByText(t.composerReadingFiles);
  fireEvent.click(screen.getByRole("button", { name: second.title, hidden: true }));
  await waitFor(() => expect(state().sessionId).toBe(second.id));
  expect((screen.getByRole("button", { name: t.composerAddFiles }) as HTMLButtonElement).disabled).toBe(false);
  fireEvent.change(picker, { target: { files: [newFile] } });
  await act(async () => finishOld(content));
  expect(screen.getByText(t.composerReadingFiles)).toBeTruthy();
  await act(async () => finishNew(content));
  await screen.findByText("new.txt");
  expect(screen.queryByText("old.txt")).toBeNull();
});

it.each([false, true])("keeps a user's draft when a plugin sends a message (running: %s)", async (active) => {
  const destination = {
    id: "studio", qualified_id: `${videoPlugin.id}:studio`, title: "Fixture studio", icon: null,
    main_page: "index.html", page_runtime: "localWeb" as const, page_url: "about:blank", page_server: null,
    sidebar: null, toolbar_commands: [], default_pinned: true, pinned_order: null,
  };
  const plugin = { ...videoPlugin, destinations: [destination] };
  const event = { session_id: restored.id, turn_id: "plugin-turn" };
  localStorage.setItem("willdeep.web.last-session./other", restored.id);
  const { container, posts } = mount({
    workspaces: [{ ...other, active: true, approval_mode_configured: true }],
    sessions: [{ ...restored, active }], plugins: [plugin], chatResponse: () => new Response(
      `data: ${JSON.stringify({ ...event, type: "submitted" })}\n\ndata: ${JSON.stringify({ ...event, type: "completed", text: "Fixture complete" })}\n\n`,
    ),
  });
  await ready("/other");
  await waitFor(() => expect(state().sessionId).toBe(restored.id));
  const draft = "My own draft";
  const composer = container.querySelector("textarea")!;
  fireEvent.change(composer, { target: { value: draft } });
  fireEvent.paste(composer, { clipboardData: { files: [], items: [], getData: () => pastedAttachmentText } });
  const attachmentName = `${t.pastedText}-1.txt`;
  await screen.findByText(attachmentName);
  fireEvent.click(await screen.findByRole("button", { name: destination.title }));
  const frame = container.querySelector<HTMLIFrameElement>("iframe.plugin-frame");
  if (!frame) throw new Error("Expected the plugin frame to be mounted");
  expect(frame.title).toBe(destination.title);
  const text = "Summarize the studio notes";
  act(() => {
    window.dispatchEvent(new MessageEvent("message", {
      source: frame.contentWindow, origin: "null", data: { __willdeep: 1, type: "chatSend", requestID: "fixture", text },
    }));
  });
  await waitFor(() => expect(posts).toHaveLength(1));
  expect(posts[0]).toMatchObject({ url: active ? `/api/sessions/${restored.id}/queue` : "/api/chat/stream", body: { prompt: text, attachments: [] } });
  await screen.findByText(attachmentName);
  expect(container.querySelector("textarea")?.value).toBe(draft);
});

it.each([false, true])("checks the full UTF-8 request before submission (queue: %s)", async (queue) => {
  localStorage.setItem("willdeep.web.last-session./other", restored.id);
  const { container, posts } = mount({ workspaces: [{ ...other, active: true, approval_mode_configured: true }], sessions: [restored] });
  await waitFor(() => expect(state().sessionId).toBe(restored.id));
  const composer = container.querySelector("textarea")!;
  fireEvent.change(composer, { target: { value: "字".repeat(350_000) } });
  fireEvent.keyDown(composer, { key: "Enter", ctrlKey: queue });
  await screen.findByText(/请求大小上限/);
  expect(posts).toHaveLength(0);
  expect(composer.value).toHaveLength(350_000);
});

const inlinePastedText = "😀\n字\n".repeat(25);
const pastedAttachmentText = `${inlinePastedText}字`;

it("lets the browser paste 100 Unicode characters directly into the composer", async () => {
  const { container } = mount({ workspaces: [first] });
  await ready();
  const composer = container.querySelector("textarea")!;
  expect(fireEvent.paste(composer, { clipboardData: { files: [], items: [], getData: () => inlinePastedText } })).toBe(true);
  expect(screen.queryByText(`${t.pastedText}-1.txt`)).toBeNull();
});

function withoutRandomUUID() {
  const platformCrypto = globalThis.crypto;
  vi.stubGlobal("crypto", { getRandomValues: platformCrypto.getRandomValues.bind(platformCrypto) });
  expect("randomUUID" in globalThis.crypto).toBe(false);
}

it.each(["Enter", "button"])("%s submits chat with attachments when randomUUID is unavailable", async (trigger) => {
  withoutRandomUUID();
  const { posts, container } = mount({ workspaces: [first] });
  await ready();
  const composer = container.querySelector("textarea")!;
  fireEvent.change(composer, { target: { value: "HTTP send regression" } });
  fireEvent.paste(composer, { clipboardData: { files: [], items: [], getData: () => pastedAttachmentText } });
  if (trigger === "Enter") fireEvent.keyDown(composer, { key: "Enter" });
  else fireEvent.click(screen.getByRole("button", { name: t.send }));
  await waitFor(() => expect(posts).toHaveLength(1));
  expect(posts[0]).toMatchObject({ url: "/api/chat/stream", body: { prompt: "HTTP send regression", workspace: "/first", attachments: [{ kind: "text", content: pastedAttachmentText }] } });
  expect(posts[0].body.request_id).toMatch(/^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/);
});

it("keeps a goal set while a chat request is awaiting its submitted event", async () => {
  localStorage.setItem("willdeep.web.last-session./other", restored.id);
  let finishChat!: (response: Response) => void;
  const chatResponse = new Promise<Response>((resolve) => { finishChat = resolve; });
  const { posts, container } = mount({ workspaces: [{ ...other, active: true, approval_mode_configured: true }], sessions: [restored], chatResponse });
  await waitFor(() => expect(state().sessionId).toBe(restored.id));
  const composer = container.querySelector("textarea")!;
  fireEvent.change(composer, { target: { value: "开始聊天" } });
  fireEvent.keyDown(composer, { key: "Enter" });
  await waitFor(() => expect(posts).toHaveLength(1));
  expect(screen.getByRole("button", { name: t.stop })).toBeTruthy();

  fireEvent.click(screen.getByRole("checkbox", { name: t.goalMode }));
  const editor = screen.getByRole("textbox", { name: t.goalMode });
  fireEvent.focus(editor);
  fireEvent.change(editor, { target: { value: "持续检查测试" } });
  fireEvent.blur(editor);
  expect(screen.getByText("持续检查测试")).toBeTruthy();

  const event = { session_id: restored.id, turn_id: "goal-turn" };
  finishChat(new Response(`data: ${JSON.stringify({ ...event, type: "submitted" })}\n\ndata: ${JSON.stringify({ ...event, type: "completed", text: "本轮完成" })}\n\n`));
  await screen.findByRole("button", { name: t.send });
  expect(screen.getByText("持续检查测试")).toBeTruthy();
  expect(JSON.parse(localStorage.getItem(`willdeep.web.goal.${restored.id}`) ?? "null")).toEqual({ text: "持续检查测试", enabled: true });
});

it.each([false, true])("ordinary Enter durably queues a running session (randomUUID missing: %s)", async (missingUUID) => {
  if (missingUUID) withoutRandomUUID();
  const { posts, composer } = await mountRunning();
  fireEvent.change(composer, { target: { value: "现在进度如何" } });
  fireEvent.keyDown(composer, { key: "Enter" });
  await waitFor(() => expect(posts).toHaveLength(1));
  expect(posts[0]).toMatchObject({ url: "/api/sessions/restored/queue", body: { prompt: "现在进度如何", session_id: "restored" } });
  const notice = await screen.findByText(/已保存到待发队列，尚未交给当前模型/);
  expect(notice.textContent).toContain("正在执行: list_directory");
  expect(notice.textContent).toContain("最近完成: read_file");
  expect(composer.value).toBe("");
});

it.each(["approval", "question"] as const)("ordinary Enter retains input and opens the existing %s without queueing or resolving it", async (kind) => {
  const gate: RuntimeGate = kind === "approval"
    ? { kind, id: "pending", task_id: "task", description: "读取项目目录", always_allow_available: false, parked: true }
    : { kind, id: "pending", task_id: "task", question: "选择哪个目录？", options: [], multi_select: false, parked: true };
  const { posts, composer } = await mountRunning({ ...runningActivity, gates: [gate] });
  await screen.findByText(gate.kind === "approval" ? gate.description : gate.question);
  fireEvent.change(composer, { target: { value: "先告诉我进度" } });
  fireEvent.keyDown(composer, { key: "Enter" });
  const notice = await screen.findByText(/输入与附件已保留；请处理待确认项/);
  expect(notice.textContent).toContain(kind === "approval" ? "等待审批: 读取项目目录" : "等待回答: 选择哪个目录？");
  expect(composer.value).toBe("先告诉我进度");
  expect(document.activeElement?.id).toBe("runtime-gate-pending");
  expect(posts).toEqual([]);
});

it.each(["metaKey", "ctrlKey"])("%s+Enter explicitly queues while approval is pending", async (modifier) => {
  const gate: RuntimeGate = { kind: "approval", id: "pending", task_id: "task", description: "读取目录", always_allow_available: false, parked: true };
  const { posts, composer } = await mountRunning({ ...runningActivity, gates: [gate] });
  await screen.findByText(gate.description);
  fireEvent.change(composer, { target: { value: "稍后处理" } });
  fireEvent.keyDown(composer, { key: "Enter", [modifier]: true });
  await screen.findByText(/已保存到待发队列，尚未交给当前模型/);
  expect(posts.map((post) => post.url)).toEqual(["/api/sessions/restored/queue"]);
  expect(posts[0].body.prompt).toBe("稍后处理");
  expect(composer.value).toBe("");
  expect(screen.getByRole("button", { name: t.allowOnce })).toBeTruthy();
});

it.each([false, true])("failed queue submission preserves draft, attachments and request identity (randomUUID missing: %s)", async (missingUUID) => {
  if (missingUUID) withoutRandomUUID();
  const { posts, composer } = await mountRunning(runningActivity, 1);
  fireEvent.change(composer, { target: { value: "带附件检查进度" } });
  fireEvent.paste(composer, { clipboardData: { files: [], items: [], getData: () => pastedAttachmentText } });
  fireEvent.keyDown(composer, { key: "Enter" });
  await screen.findByText(/queue unavailable/);
  expect(composer.value).toBe("带附件检查进度");
  expect(screen.getByText(new RegExp(queueText("failed", "zh-CN")))).toBeTruthy();
  fireEvent.keyDown(composer, { key: "Enter" });
  await screen.findByText(/已保存到待发队列，尚未交给当前模型/);
  expect(posts).toHaveLength(2);
  expect(posts[1].body).toEqual(posts[0].body);
  expect(posts[0].body.attachments).toEqual([{ kind: "text", name: `${t.pastedText}-1.txt`, content: pastedAttachmentText }]);
  expect(composer.value).toBe("");
});

const selectionReply = "Before read exactly after";
const selectedPassage = " read exactly ";

async function mountTranscript(chatResponse?: (signal: AbortSignal) => Response) {
  const second = { ...restored, id: "other-chat", title: "Other conversation" };
  localStorage.setItem("willdeep.web.last-session./other", restored.id);
  const view = mount({
    workspaces: [{ ...other, active: true, approval_mode_configured: true }],
    sessions: [restored, second], history: [{ role: "assistant", content: `${selectionReply}\n\n[Other link](#fixture)`, attachment_count: 0 }], chatResponse,
  });
  const message = await screen.findByText(selectionReply);
  const viewport = screen.getByRole("region", { name: t.chatTranscript });
  const composer = view.container.querySelector("textarea")!;
  return { ...view, message, viewport, composer, second };
}

function selectPassage(message: HTMLElement) {
  const range = document.createRange();
  const start = selectionReply.indexOf(selectedPassage);
  range.setStart(message.firstChild!, start);
  range.setEnd(message.firstChild!, start + selectedPassage.length);
  Object.defineProperty(range, "getBoundingClientRect", { value: () => new DOMRect() });
  const selection = window.getSelection()!;
  fireEvent(message, new MouseEvent("pointerdown", { bubbles: true, button: 0 }));
  selection.removeAllRanges();
  selection.addRange(range);
  fireEvent(message, new MouseEvent("pointerup", { bubbles: true, button: 0 }));
  return selection;
}

it("copies the exact selection without a plugin or removing the user's highlight", async () => {
  const user = userEvent.setup();
  const { message, posts } = await mountTranscript();
  const selection = selectPassage(message);
  await user.click(within(screen.getByRole("group", { name: t.chatSelectionActions })).getByRole("button", { name: t.copyCode }));
  await screen.findByText(t.copiedCode);
  expect(await navigator.clipboard.readText()).toBe(selectedPassage);
  expect(selection.toString()).toBe(selectedPassage);
  expect(posts).toEqual([]);
});

it("preserves indentation, blank lines, and long selections in Markdown quotes", () => {
  const longLine = "x".repeat(4001);
  expect(appendSelectionQuote("Original\n\n", `  first  \n\n${longLine}`)).toBe(`Original\n\n>   first  \n> \n> ${longLine}\n\n`);
});

it("allows keyboard activation and quotes the selection into the current draft without sending", async () => {
  const user = userEvent.setup();
  const { message, viewport, composer, posts } = await mountTranscript();
  fireEvent.change(composer, { target: { value: "Keep this draft\n" } });
  viewport.focus();
  selectPassage(message);
  await user.keyboard("{Shift>}{F10}{/Shift}");
  const actions = within(screen.getByRole("group", { name: t.chatSelectionActions }));
  expect(document.activeElement).toBe(actions.getByRole("button", { name: t.copyCode }));
  await user.tab();
  expect(document.activeElement).toBe(actions.getByRole("button", { name: t.chatAddToTask }));
  await user.keyboard("{Enter}");
  expect(composer.value).toBe(`Keep this draft\n\n> ${selectedPassage}\n\n`);
  await waitFor(() => expect(document.activeElement).toBe(composer));
  expect(composer.selectionStart).toBe(composer.value.length);
  expect(state().sessionId).toBe(restored.id);
  expect(posts).toEqual([]);
});

it.each(["/model local", "/name example", "/permissions smart", "/goal finish"])("submits a quoted %s draft as chat text", async (draft) => {
  const { message, composer, posts } = await mountTranscript();
  const quoted = appendSelectionQuote(draft, selectedPassage);
  fireEvent.change(composer, { target: { value: draft } });
  selectPassage(message);
  fireEvent.click(screen.getByRole("button", { name: t.chatAddToTask }));
  expect(composer.value).toBe(quoted);
  expect(posts).toEqual([]);
  fireEvent.keyDown(composer, { key: "Enter" });
  await waitFor(() => expect(posts).toMatchObject([{ url: "/api/chat/stream", body: { prompt: quoted.trim(), session_id: restored.id } }]));
});

it("dismisses selection actions when the live range shrinks to whitespace", async () => {
  const { message } = await mountTranscript();
  const selection = selectPassage(message);
  expect(screen.getByRole("group", { name: t.chatSelectionActions })).toBeTruthy();
  const range = selection.getRangeAt(0);
  range.setEnd(range.startContainer, range.startOffset + 1);
  fireEvent(document, new Event("selectionchange"));
  expect(selection.toString()).toBe(" ");
  expect(screen.queryByRole("group", { name: t.chatSelectionActions })).toBeNull();
});

it("dismisses selection actions on nested scroll and keeps the highlight", async () => {
  const { message } = await mountTranscript();
  const selection = selectPassage(message);
  expect(screen.getByRole("group", { name: t.chatSelectionActions })).toBeTruthy();
  fireEvent.scroll(message);
  expect(screen.queryByRole("group", { name: t.chatSelectionActions })).toBeNull();
  expect(selection.toString()).toBe(selectedPassage);
});

it("preserves the native context menu of an unrelated link", async () => {
  const { message } = await mountTranscript();
  const link = screen.getByRole("link", { name: "Other link" });
  link.focus();
  const selection = selectPassage(message);
  expect(selection.toString()).toBe(selectedPassage);
  expect(fireEvent.keyDown(link, { shiftKey: true, key: "F10" })).toBe(true);
  expect(fireEvent.contextMenu(link)).toBe(true);
  expect(document.activeElement).toBe(link);
  expect(fireEvent.contextMenu(message)).toBe(false);
});

it("dismisses selection controls on scroll, Escape, and session change", async () => {
  const { message, viewport, second } = await mountTranscript();
  const selection = selectPassage(message);
  expect(screen.getByRole("group", { name: t.chatSelectionActions })).toBeTruthy();
  fireEvent.scroll(viewport);
  expect(screen.queryByRole("group", { name: t.chatSelectionActions })).toBeNull();
  expect(selection.toString()).toBe(selectedPassage);

  viewport.focus();
  fireEvent.keyDown(viewport, { key: "F10", shiftKey: true });
  fireEvent.keyDown(document.activeElement!, { key: "Escape" });
  expect(document.activeElement).toBe(viewport);
  expect(screen.queryByRole("group", { name: t.chatSelectionActions })).toBeNull();

  selectPassage(message);
  fireEvent.click(screen.getByRole("button", { name: second.title, hidden: true }));
  await waitFor(() => expect(state().sessionId).toBe(second.id));
  expect(screen.queryByRole("group", { name: t.chatSelectionActions })).toBeNull();
});

it("shows the rocket only away from the bottom and resumes following assistant output after a jump", async () => {
  let stream!: ReadableStreamDefaultController<Uint8Array>;
  const resizeCallbacks = new Map<Element, () => void>();
  vi.stubGlobal("ResizeObserver", class {
    constructor(private callback: () => void) {}
    observe(target: Element) { resizeCallbacks.set(target, this.callback); }
    unobserve() {}
    disconnect() {}
  });
  const { viewport, composer } = await mountTranscript(() => new Response(new ReadableStream<Uint8Array>({ start(controller) { stream = controller; } })));
  const content = viewport.querySelector(".chat-timeline")!.parentElement!;
  let height = 1000;
  let top = 600;
  Object.defineProperties(viewport, {
    scrollHeight: { get: () => height },
    clientHeight: { value: 300 },
    scrollTop: { get: () => top, set: (value: number) => { top = Math.min(value, height - viewport.clientHeight); } },
  });
  const latest = () => screen.queryByRole("button", { name: t.chatJumpToLatest });
  fireEvent.scroll(viewport);
  expect(latest()).toBeNull();

  fireEvent.change(composer, { target: { value: "Continue the fixture reply" } });
  fireEvent.click(screen.getByRole("button", { name: t.send }));
  await waitFor(() => expect(stream).toBeDefined());
  top = 400;
  fireEvent.scroll(viewport);
  expect(latest()).toBeTruthy();

  const event = (payload: object) => stream.enqueue(new TextEncoder().encode(`data: ${JSON.stringify(payload)}\n\n`));
  height = 1300;
  event({ type: "assistant_text_delta", text: "While browsing history" });
  await screen.findByText("While browsing history");
  expect(top).toBe(400);

  fireEvent.click(latest()!);
  expect(top).toBe(1000);
  expect(latest()).toBeNull();
  expect(document.activeElement).toBe(viewport);

  event({ type: "assistant_text_delta", text: " and after the jump" });
  await screen.findByText("While browsing history and after the jump");
  height = 1600;
  act(() => resizeCallbacks.get(content)!());
  expect(top).toBe(1300);
  expect(latest()).toBeNull();

  top = 950;
  fireEvent.scroll(viewport);
  height = 1900;
  act(() => resizeCallbacks.get(content)!());
  expect(top).toBe(950);
  expect(latest()).toBeTruthy();
  stream.close();
});
