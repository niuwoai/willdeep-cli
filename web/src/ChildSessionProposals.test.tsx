// @vitest-environment jsdom
import { afterEach, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { ChildSessions } from "./ChildSessions";
import { childProposalsKey } from "./childProposalQueries";
import type { ChildProposal, ChildTask } from "./childSessionsApi";
import type { Language } from "./i18n";

const parent = "proposal-parent";
const proposal = (params: Partial<ChildProposal["params"]> = {}): ChildProposal => ({
  id: "proposal-id", created_at: 10, dismissed: false,
  params: { parent_session_id: parent, created_from_turn_id: "source-turn", title: "调查恢复流程", prompt: "检查恢复入口和状态变化。\n报告问题与验证结果。",
    context: "fresh", workspace_mode: "shared_read_only", worktree_base: "head", notify: "on_task_terminal", ...params },
});
const task = (): ChildTask => ({
  assignment_id: "assignment-id", parent_session_id: parent, child_session_id: "child-id", child_turn_id: "child-turn",
  title: "调查恢复流程", prompt: "检查恢复入口和状态变化。\n报告问题与验证结果。", context: "fresh", workspace_mode: "shared_read_only",
  status: "queued", created_at: 11, updated_at: 11, model: null, through_turn_id: null, result: null, error: null,
});
type Posted = { path: string; body: Record<string, unknown> };
function mockApi(initial: ChildProposal[] = [proposal()]) {
  const state = { proposals: initial, tasks: [] as ChildTask[], posts: [] as Posted[], proposalStatus: 200,
    onPost: null as ((post: Posted) => Promise<Response> | Response) | null };
  const fetcher = vi.fn(async (path: string, init?: RequestInit) => {
    if (init?.method === "POST") {
      const post = { path, body: JSON.parse(String(init.body)) as Record<string, unknown> };
      state.posts.push(post);
      if (state.onPost) return state.onPost(post);
      state.proposals = [];
      if (path.endsWith("/dismiss")) return Response.json({ proposal_id: "proposal-id", status: "dismissed" });
      state.tasks = [task()];
      return Response.json(state.tasks[0]);
    }
    if (path.endsWith("/proposals")) return Response.json(state.proposalStatus === 200 ? { proposals: state.proposals } : { error: "unavailable" }, { status: state.proposalStatus });
    if (path.endsWith("/capabilities")) return Response.json({ capabilities: [] });
    return Response.json({ tasks: state.tasks, next_cursor: null });
  });
  vi.stubGlobal("fetch", fetcher);
  return { state, fetcher };
}
function mount(language: Language = "zh-CN") {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } });
  const open = vi.fn();
  const view = render(<QueryClientProvider client={client}>
    <textarea aria-label="父会话草稿" defaultValue="保留当前输入" />
    <ChildSessions sessionId={parent} language={language} onOpenSession={open} />
  </QueryClientProvider>);
  return { ...view, client, open };
}
afterEach(() => { cleanup(); vi.unstubAllGlobals(); });

it("does not render a creation panel while loading or after ordinary chat has no proposals or tasks", async () => {
  const { state } = mockApi([]);
  const { client } = mount();
  expect(screen.queryByRole("region", { name: "子会话" })).toBeNull();
  await waitFor(() => expect(client.getQueryState(childProposalsKey(parent))?.status).toBe("success"));
  expect(screen.queryByRole("region", { name: "子会话" })).toBeNull();
  expect(screen.queryByText("发起子会话")).toBeNull();
  expect(screen.queryByText(/暂无子会话任务/)).toBeNull();
  expect(state.posts).toHaveLength(0);
});

it("renders a complete filled proposal without input fields or automatically starting work", async () => {
  const { state } = mockApi([proposal({ context: "fork", through_turn_id: "completed-turn", workspace_mode: "worktree", worktree_base: "snapshot", model: "worker-model", subscription: "inbox", depends_on: ["prerequisite-a", "prerequisite-b"] })]);
  state.tasks = [
    { ...task(), assignment_id: "prerequisite-a", title: "检查权限" },
    { ...task(), assignment_id: "prerequisite-b", title: "检查依赖" },
  ];
  mount();
  const card = (await screen.findByText("调查恢复流程", { selector: "h3" })).closest("li")!;
  expect(within(card).getByText(/检查恢复入口和状态变化/).textContent).toBe(proposal().params.prompt);
  expect(within(card).queryByRole("textbox")).toBeNull();
  expect(within(card).queryByRole("combobox")).toBeNull();
  expect(within(card).getByText(/独立工作树 · 可写 · 继承已完成历史/)).toBeTruthy();
  fireEvent.click(within(card).getByText("执行选项"));
  for (const text of ["worker-model", "仅保留在任务列表", "冻结当前未提交修改", "检查权限", "检查依赖"]) expect(within(card).getByText(text)).toBeTruthy();
  expect(within(card).getByRole("button", { name: "开始" })).toBeTruthy();
  expect(state.posts).toHaveLength(0);
});

it("starts with only proposal identity, refreshes tasks and preserves the parent draft and focus", async () => {
  const { state } = mockApi();
  const { open } = mount();
  fireEvent.click(await screen.findByRole("button", { name: "开始" }));
  screen.getByLabelText("父会话草稿").focus();
  await screen.findByText("子会话任务已创建，你可以继续当前对话。");
  expect(state.posts).toHaveLength(1);
  expect(state.posts[0].path).toBe(`/api/sessions/${parent}/children/proposals/proposal-id/start`);
  expect(Object.keys(state.posts[0].body)).toEqual(["request_id"]);
  expect(state.posts[0].body.request_id).toBeTypeOf("string");
  expect(screen.queryByRole("button", { name: "开始" })).toBeNull();
  expect(await screen.findByRole("button", { name: "打开完整对话" })).toBeTruthy();
  expect((screen.getByLabelText("父会话草稿") as HTMLTextAreaElement).value).toBe("保留当前输入");
  expect(document.activeElement).toBe(screen.getByLabelText("父会话草稿"));
  expect(open).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole("button", { name: "打开完整对话" }));
  expect(open).toHaveBeenCalledWith("child-id");
});

it("retains the complete proposal and reuses its request key after a lost response", async () => {
  const { state } = mockApi();
  state.onPost = () => Response.json({ error: "response lost" }, { status: 503 });
  const { open } = mount();
  fireEvent.click(await screen.findByRole("button", { name: "开始" }));
  await screen.findByRole("alert");
  expect(screen.getByText("调查恢复流程", { selector: "h3" })).toBeTruthy();
  expect(screen.getByText(/检查恢复入口和状态变化/).textContent).toBe(proposal().params.prompt);
  state.onPost = null;
  fireEvent.click(screen.getByRole("button", { name: "开始" }));
  await screen.findByText("子会话任务已创建，你可以继续当前对话。");
  expect(state.posts).toHaveLength(2);
  expect(state.posts[0]).toEqual(state.posts[1]);
  expect(open).not.toHaveBeenCalled();
});

it("blocks repeated clicks and dismissal while the start request is in flight", async () => {
  const { state } = mockApi();
  let respond: (response: Response) => void = () => {};
  state.onPost = () => new Promise<Response>((resolve) => { respond = resolve; });
  mount();
  const start = await screen.findByRole("button", { name: "开始" });
  await act(async () => { fireEvent.click(start); fireEvent.click(start); });
  await waitFor(() => expect(state.posts).toHaveLength(1));
  expect((screen.getByRole("button", { name: "正在开始…" }) as HTMLButtonElement).disabled).toBe(true);
  const dismiss = screen.getByRole("button", { name: "不创建" }) as HTMLButtonElement;
  expect(dismiss.disabled).toBe(true);
  fireEvent.click(dismiss);
  expect(state.posts).toHaveLength(1);
  state.proposals = []; state.tasks = [task()];
  await act(async () => { respond(Response.json(task())); });
  await screen.findByRole("button", { name: "打开完整对话" });
});

it("shows persisted preparation failure after reload and still offers start or dismiss", async () => {
  const { state } = mockApi([{ ...proposal(), error: "Fork boundary no longer matches the conversation compression generation" }]);
  mount();
  expect((await screen.findByRole("alert")).textContent).toContain("compression generation");
  expect(screen.getByRole("button", { name: "开始" })).toBeTruthy();
  fireEvent.click(screen.getByRole("button", { name: "不创建" }));
  await waitFor(() => expect(screen.queryByRole("region", { name: "子会话" })).toBeNull());
  expect(state.tasks).toHaveLength(0);
});

it("restores a pending proposal after remount without submitting anything until clicked", async () => {
  const { state } = mockApi();
  const first = mount();
  await screen.findByRole("button", { name: "开始" });
  first.unmount(); first.client.clear();
  mount();
  expect(await screen.findByText("调查恢复流程", { selector: "h3" })).toBeTruthy();
  expect(state.posts).toHaveLength(0);
  fireEvent.click(screen.getByRole("button", { name: "开始" }));
  await screen.findByText("子会话任务已创建，你可以继续当前对话。");
  expect(state.posts).toHaveLength(1);
});

it("reconciles a proposal started elsewhere on refresh without creating another assignment", async () => {
  const { state } = mockApi();
  const { client } = mount();
  await screen.findByRole("button", { name: "开始" });
  state.proposals = []; state.tasks = [task()];
  fireEvent.click(screen.getByRole("button", { name: "刷新" }));
  await screen.findByRole("button", { name: "打开完整对话" });
  await waitFor(() => expect(client.getQueryData(childProposalsKey(parent))).toEqual({ proposals: [] }));
  expect(screen.queryByRole("button", { name: "开始" })).toBeNull();
  expect(state.posts).toHaveLength(0);
});

it("dismisses a proposal without creating a task and then hides the empty panel", async () => {
  const { state } = mockApi();
  mount();
  fireEvent.click(await screen.findByRole("button", { name: "不创建" }));
  await waitFor(() => expect(screen.queryByRole("region", { name: "子会话" })).toBeNull());
  expect(state.posts).toHaveLength(1);
  expect(state.posts[0].path).toBe(`/api/sessions/${parent}/children/proposals/proposal-id/dismiss`);
  expect(Object.keys(state.posts[0].body)).toEqual(["request_id"]);
  expect(state.tasks).toHaveLength(0);
});

it("offers retry when loading proposals fails and recovers the filled card", async () => {
  const { state } = mockApi(); state.proposalStatus = 503;
  mount();
  await screen.findByRole("alert");
  expect(screen.queryByRole("button", { name: "开始" })).toBeNull();
  state.proposalStatus = 200;
  fireEvent.click(screen.getByRole("button", { name: "重试加载" }));
  expect(await screen.findByRole("button", { name: "开始" })).toBeTruthy();
  expect(screen.queryByRole("alert")).toBeNull();
  expect(state.posts).toHaveLength(0);
});

it.each(["en", "ja"] as const)("localizes proposal actions in %s", async (language) => {
  mockApi(); mount(language);
  expect(await screen.findByRole("button", { name: language === "en" ? "Start" : "開始" })).toBeTruthy();
  expect(screen.queryByText("等待开始")).toBeNull();
  expect(screen.queryByText("不创建")).toBeNull();
});
