// @vitest-environment jsdom
import { afterEach, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { ChildSessionOrigin, ChildSessions } from "./ChildSessions";
import { CHILD_CAPABILITIES, type ChildLink, type ChildTask } from "./childSessionsApi";
import { childTasksKey } from "./childSessionsQueries";

const CAPABILITIES = Object.values(CHILD_CAPABILITIES);
const result = { revision: 3, summary: "调查结论", report: "任务报告", report_complete: true, consumed_at: null, superseded_by: null };
const task = (overrides: Partial<ChildTask> = {}): ChildTask => ({
  assignment_id: "assignment-a", parent_session_id: "parent", child_session_id: "child-a", child_turn_id: "turn-a",
  title: "检查恢复", prompt: "调查恢复流程", status: "completed", context: "fresh", workspace_mode: "shared_read_only",
  model: null, through_turn_id: null, created_at: 1, updated_at: 1, result, error: null,
  control_state: "delegated", control_revision: 4, subscription: "auto_wake", subscription_revision: 6,
  depends_on: [], waiting_on_dependencies: false, ...overrides,
});
type Posted = { path: string; body: Record<string, unknown> };
function mockApi(parent: string, tasks: ChildTask[]) {
  const state = {
    tasks: tasks.map((value) => ({ ...value, parent_session_id: parent })),
    capabilities: [...CAPABILITIES] as string[], capabilitiesStatus: 200, linkStatus: 200,
    link: { parent_session_id: parent, child_session_id: "child-a", control_state: "delegated", control_revision: 4 } as ChildLink,
    posts: [] as Posted[], onPost: null as ((request: Posted) => Response | Promise<Response>) | null,
    onLink: null as (() => Response | Promise<Response>) | null,
  };
  const fetcher = vi.fn(async (path: string, init?: RequestInit) => {
    if (init?.method === "POST") {
      const request = { path, body: JSON.parse(String(init.body)) as Record<string, unknown> };
      state.posts.push(request);
      return state.onPost ? state.onPost(request) : Response.json(state.tasks[0]);
    }
    if (path.endsWith("/capabilities")) return Response.json({ capabilities: state.capabilities }, { status: state.capabilitiesStatus });
    if (path.includes("/links/")) return state.onLink ? state.onLink() : Response.json(state.link, { status: state.linkStatus });
    return Response.json({ tasks: state.tasks, next_cursor: null });
  });
  vi.stubGlobal("fetch", fetcher);
  return { state, fetcher };
}
function mount(parent: string) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } });
  const open = vi.fn(); const changed = vi.fn();
  const rendered = render(<QueryClientProvider client={client}>
    <textarea aria-label="父会话草稿" defaultValue="保留父聊天草稿" />
    <ChildSessions sessionId={parent} language="zh-CN" onOpenSession={open} onRelationshipChanged={changed} />
  </QueryClientProvider>);
  return { ...rendered, client, open, changed };
}
const fieldValue = (label: string) => (screen.getByLabelText(label) as HTMLInputElement).value;
const subscriptionSelect = async () => within((await screen.findByText("检查恢复", { selector: "strong" })).closest("li")!).findByLabelText("结果通知");
const fillDraft = () => {
  fireEvent.click(screen.getByText("发起子会话"));
  fireEvent.change(screen.getByLabelText("任务名称"), { target: { value: "后续调查" } });
  fireEvent.change(screen.getByLabelText("任务说明"), { target: { value: "保留子任务草稿" } });
};
afterEach(() => { cleanup(); vi.unstubAllGlobals(); });

it("requires a fresh Link and explicit confirmation, and preserves drafts and focus after takeover", async () => {
  const { state, fetcher } = mockApi("p2-takeover", [task()]);
  const { open, changed } = mount("p2-takeover");
  await screen.findByRole("button", { name: "由我接管" });
  fillDraft();
  fireEvent.click(screen.getByRole("button", { name: "继续任务" }));
  fireEvent.change(screen.getByLabelText("后续任务"), { target: { value: "保留续做草稿" } });
  fireEvent.click(screen.getByRole("button", { name: "由我接管" }));
  const dialog = await screen.findByRole("dialog", { name: "确认接管" });
  expect(state.posts).toHaveLength(0);
  expect(within(dialog).getByText(/当前执行会继续/)).toBeTruthy();
  await waitFor(() => expect((within(dialog).getByRole("button", { name: "确认接管" }) as HTMLButtonElement).disabled).toBe(false));
  state.onPost = () => {
    state.link = { ...state.link, control_state: "user_owned", control_revision: 5 };
    state.tasks = state.tasks.map((value) => ({ ...value, control_state: "user_owned", control_revision: 5 }));
    return Response.json(state.link);
  };
  fireEvent.click(within(dialog).getByRole("button", { name: "确认接管" }));
  screen.getByLabelText("父会话草稿").focus();
  await screen.findByText("会话关系已更新。聊天、任务历史与草稿已保留。");
  expect(state.posts).toHaveLength(1);
  expect(state.posts[0]).toMatchObject({ path: "/api/sessions/p2-takeover/children/links/child-a/takeover", body: { expected_control_revision: 4 } });
  expect(state.posts[0].body.request_id).toBeTypeOf("string");
  expect(fetcher.mock.calls.some(([path, init]) => path.endsWith("/links/child-a") && !init?.method)).toBe(true);
  expect(fieldValue("任务名称")).toBe("后续调查"); expect(fieldValue("任务说明")).toBe("保留子任务草稿");
  expect(fieldValue("后续任务")).toBe("保留续做草稿"); expect(fieldValue("父会话草稿")).toBe("保留父聊天草稿");
  expect(document.activeElement).toBe(screen.getByLabelText("父会话草稿"));
  expect((screen.getByRole("button", { name: "继续任务" }) as HTMLButtonElement).disabled).toBe(true);
  expect((screen.getByRole("button", { name: "提交后续任务" }) as HTMLButtonElement).disabled).toBe(true);
  expect(screen.queryByRole("button", { name: "由我接管" })).toBeNull();
  expect(screen.getByRole("button", { name: "解除父子关系" })).toBeTruthy();
  expect(open).not.toHaveBeenCalled(); expect(changed).toHaveBeenCalledWith(state.link);
});

it("can cancel detachment and retains the same request identity when its response is lost", async () => {
  const { state } = mockApi("p2-detach-replay", [task({ control_state: "user_owned", control_revision: 5 })]);
  state.link = { ...state.link, control_state: "user_owned", control_revision: 5 };
  mount("p2-detach-replay");
  fireEvent.click(await screen.findByRole("button", { name: "解除父子关系" }));
  let dialog = await screen.findByRole("dialog", { name: "确认解除关系" });
  fireEvent.click(within(dialog).getByRole("button", { name: "取消" }));
  expect(state.posts).toHaveLength(0);
  state.onPost = () => Response.json({ error: "response lost" }, { status: 503 });
  fireEvent.click(screen.getByRole("button", { name: "解除父子关系" }));
  dialog = await screen.findByRole("dialog", { name: "确认解除关系" });
  await waitFor(() => expect((within(dialog).getByRole("button", { name: "确认解除关系" }) as HTMLButtonElement).disabled).toBe(false));
  fireEvent.click(within(dialog).getByRole("button", { name: "确认解除关系" }));
  await within(dialog).findByRole("alert");
  state.onPost = () => {
    state.link = { ...state.link, control_state: "detached", control_revision: 6 };
    state.tasks = state.tasks.map((value) => ({ ...value, control_state: "detached", control_revision: 6 }));
    return Response.json(state.link);
  };
  fireEvent.click(within(dialog).getByRole("button", { name: "确认解除关系" }));
  await screen.findByText("会话关系已更新。聊天、任务历史与草稿已保留。");
  expect(state.posts[0].body).toEqual(state.posts[1].body);
  expect(state.posts[0]).toMatchObject({ path: "/api/sessions/p2-detach-replay/children/links/child-a/detach", body: { expected_control_revision: 5 } });
  expect(screen.queryByRole("button", { name: "解除父子关系" })).toBeNull();
});

it("refreshes a conflicting control revision before allowing another confirmation", async () => {
  const { state } = mockApi("p2-conflict", [task()]);
  mount("p2-conflict");
  fireEvent.click(await screen.findByRole("button", { name: "由我接管" }));
  const dialog = await screen.findByRole("dialog");
  await waitFor(() => expect((within(dialog).getByRole("button", { name: "确认接管" }) as HTMLButtonElement).disabled).toBe(false));
  state.onPost = () => {
    state.link = { ...state.link, control_state: "user_owned", control_revision: 9 };
    state.tasks = state.tasks.map((value) => ({ ...value, control_state: "user_owned", control_revision: 9 }));
    return Response.json({ error: "stale revision" }, { status: 409 });
  };
  fireEvent.click(within(dialog).getByRole("button", { name: "确认接管" }));
  await within(dialog).findByText(/状态已被其他操作更新/);
  await waitFor(() => expect((within(dialog).getByRole("button", { name: "确认接管" }) as HTMLButtonElement).disabled).toBe(true));
  expect((screen.getByRole("button", { name: "继续任务" }) as HTMLButtonElement).disabled).toBe(true);
  expect(state.posts).toHaveLength(1);
});

it("keeps confirmation disabled while reading the Link and after a Link error", async () => {
  const { state, fetcher } = mockApi("p2-link-error", [task()]);
  let respond: (response: Response) => void = () => {};
  state.onLink = () => new Promise<Response>((resolve) => { respond = resolve; });
  mount("p2-link-error");
  fireEvent.click(await screen.findByRole("button", { name: "由我接管" }));
  const dialog = await screen.findByRole("dialog");
  expect(within(dialog).getByText("正在核对当前会话关系…")).toBeTruthy();
  expect((within(dialog).getByRole("button", { name: "确认接管" }) as HTMLButtonElement).disabled).toBe(true);
  await waitFor(() => expect(fetcher.mock.calls.some(([path]) => path.endsWith("/links/child-a"))).toBe(true));
  respond(Response.json({ error: "missing" }, { status: 404 }));
  await within(dialog).findByRole("alert");
  expect((within(dialog).getByRole("button", { name: "确认接管" }) as HTMLButtonElement).disabled).toBe(true);
  expect(state.posts).toHaveLength(0);
});

it.each(["user_owned", "detached"] as const)("disables parent stop/retry/continue for %s without hiding chat or history", async (control_state) => {
  const { state } = mockApi(`p2-disabled-${control_state}`, [
    task({ control_state, status: "failed" }),
    task({ assignment_id: "running-task", child_session_id: "running-child", control_state, status: "running", result: null }),
  ]);
  const { open } = mount(`p2-disabled-${control_state}`);
  await screen.findByText("本次任务失败");
  for (const name of ["重试任务", "继续任务", "停止本次任务"]) expect((screen.getByRole("button", { name }) as HTMLButtonElement).disabled).toBe(true);
  expect(screen.getByText("任务报告")).toBeTruthy();
  fireEvent.click(screen.getAllByRole("button", { name: "打开完整对话" })[0]);
  expect(open).toHaveBeenCalledWith("child-a"); expect(state.posts).toHaveLength(0);
});

it("starts a task with multiple assignment prerequisites and an inbox subscription", async () => {
  const { state } = mockApi("p2-start-options", [task(), task({ assignment_id: "assignment-b", title: "检查权限", child_session_id: "child-b" })]);
  mount("p2-start-options");
  await screen.findAllByRole("button", { name: "由我接管" });
  fillDraft();
  const form = screen.getByLabelText("任务名称").closest("form")!;
  fireEvent.change(within(form).getByLabelText("结果通知"), { target: { value: "inbox" } });
  fireEvent.click(within(form).getByRole("checkbox", { name: /检查恢复/ }));
  fireEvent.click(within(form).getByRole("checkbox", { name: /检查权限/ }));
  fireEvent.click(screen.getByRole("button", { name: "开始任务" }));
  await waitFor(() => expect(state.posts).toHaveLength(1));
  expect(state.posts[0].body).toMatchObject({ subscription: "inbox", depends_on: ["assignment-a", "assignment-b"] });
});

it("sends dependency and notification choices when continuing the same conversation", async () => {
  const { state } = mockApi("p2-continue-options", [task()]);
  mount("p2-continue-options");
  fireEvent.click(await screen.findByRole("button", { name: "继续任务" }));
  const form = screen.getByLabelText("后续任务").closest("form")!;
  fireEvent.change(screen.getByLabelText("后续任务"), { target: { value: "根据上一轮结果继续" } });
  fireEvent.change(within(form).getByLabelText("结果通知"), { target: { value: "none" } });
  fireEvent.click(within(form).getByRole("checkbox", { name: /检查恢复/ }));
  fireEvent.click(screen.getByRole("button", { name: "提交后续任务" }));
  await waitFor(() => expect(state.posts).toHaveLength(1));
  expect(state.posts[0]).toMatchObject({ path: "/api/sessions/p2-continue-options/children/child-a/continue", body: { subscription: "none", depends_on: ["assignment-a"], prompt: "根据上一轮结果继续" } });
});

it("shows dependency waiting separately from preparation, with recognizable assignment IDs and titles", async () => {
  mockApi("p2-dependency-status", [task(), task({ assignment_id: "waiting", child_session_id: "waiting-child", title: "汇总结论", status: "preparing", result: null, child_turn_id: null, waiting_on_dependencies: true, depends_on: ["assignment-a", "unloaded-dependency"] })]);
  mount("p2-dependency-status");
  await screen.findByText("等待前置任务", { selector: ".child-task-state" });
  const history = screen.getByText("依赖的任务").parentElement!;
  expect(within(history).getByText("assignment-a")).toBeTruthy();
  expect(within(history).getByText("检查恢复")).toBeTruthy();
  expect(within(history).getByText("unloaded-dependency")).toBeTruthy();
  expect(within(history).getByText("未加载的任务")).toBeTruthy();
  expect(screen.queryByText("准备中")).toBeNull();
});

it("limits prerequisite selection to 16 and excludes conversations outside parent control", async () => {
  mockApi("p2-dependency-limit", [...Array.from({ length: 17 }, (_, index) => task({ assignment_id: `dep-${index}`, child_session_id: `child-${index}`, title: `前置 ${index}` })), task({ assignment_id: "owned", child_session_id: "owned-child", title: "已接管前置", control_state: "user_owned" })]);
  mount("p2-dependency-limit");
  await screen.findByText("已接管前置"); fillDraft();
  const form = screen.getByLabelText("任务名称").closest("form")!;
  await waitFor(() => expect(within(form).getAllByRole("checkbox")).toHaveLength(17));
  const boxes = within(form).getAllByRole("checkbox") as HTMLInputElement[];
  boxes.slice(0, 16).forEach((box) => fireEvent.click(box));
  expect(boxes[16].disabled).toBe(true);
  fireEvent.click(boxes[0]); expect(boxes[16].disabled).toBe(false);
  expect(within(form).queryByRole("checkbox", { name: /已接管前置/ })).toBeNull();
});

it("keeps a selected prerequisite visible for removal after it is taken over", async () => {
  const { state } = mockApi("p2-dependency-stale", [task()]);
  const { client } = mount("p2-dependency-stale");
  await screen.findByRole("button", { name: "由我接管" }); fillDraft();
  const form = screen.getByLabelText("任务名称").closest("form")!;
  fireEvent.click(within(form).getByRole("checkbox", { name: /检查恢复/ }));
  state.tasks = state.tasks.map((value) => ({ ...value, control_state: "user_owned" }));
  await client.invalidateQueries({ queryKey: childTasksKey("p2-dependency-stale") });
  await within(form).findByText(/部分已选前置任务不再受父会话管理/);
  expect((screen.getByRole("button", { name: "开始任务" }) as HTMLButtonElement).disabled).toBe(true);
  fireEvent.click(within(form).getByRole("checkbox", { name: /检查恢复/ }));
  expect((screen.getByRole("button", { name: "开始任务" }) as HTMLButtonElement).disabled).toBe(false);
});

it("updates only the assignment subscription with its expected revision, preserving drafts", async () => {
  const { state } = mockApi("p2-subscription", [task()]);
  mount("p2-subscription");
  const select = await subscriptionSelect();
  state.onPost = () => {
    state.tasks = state.tasks.map((value) => ({ ...value, subscription: "inbox", subscription_revision: 7 }));
    return Response.json(state.tasks[0]);
  };
  fireEvent.change(select, { target: { value: "inbox" } });
  fireEvent.click(screen.getByRole("button", { name: "保存通知偏好" }));
  screen.getByLabelText("父会话草稿").focus();
  await screen.findByText("本次任务的通知偏好已更新。");
  expect(state.posts[0]).toMatchObject({ path: "/api/sessions/p2-subscription/children/assignment-a/subscription", body: { subscription: "inbox", expected_subscription_revision: 6 } });
  expect(fieldValue("父会话草稿")).toBe("保留父聊天草稿");
  expect(document.activeElement).toBe(screen.getByLabelText("父会话草稿"));
  expect(screen.getByText("仅在任务列表保留")).toBeTruthy();
});

it("refreshes subscription conflicts and retries with the new revision, retaining the user's choice", async () => {
  const { state } = mockApi("p2-subscription-conflict", [task()]);
  mount("p2-subscription-conflict");
  const select = await subscriptionSelect();
  state.onPost = () => {
    state.tasks = state.tasks.map((value) => ({ ...value, subscription_revision: 12 }));
    return Response.json({ error: "stale subscription" }, { status: 409 });
  };
  fireEvent.change(select, { target: { value: "none" } });
  fireEvent.click(screen.getByRole("button", { name: "保存通知偏好" }));
  await screen.findByText(/状态已被其他操作更新/);
  await waitFor(() => expect((screen.getByRole("button", { name: "保存通知偏好" }) as HTMLButtonElement).disabled).toBe(false));
  state.onPost = () => Response.json({ ...state.tasks[0], subscription: "none", subscription_revision: 13 });
  fireEvent.click(screen.getByRole("button", { name: "保存通知偏好" }));
  await waitFor(() => expect(state.posts).toHaveLength(2));
  expect(state.posts[1].body).toMatchObject({ subscription: "none", expected_subscription_revision: 12 });
  expect(state.posts[0].body.request_id).not.toBe(state.posts[1].body.request_id);
});

it.each([
  ["none", null, "通知已关闭"], ["inbox", null, "仅在任务列表保留"], ["auto_wake", null, "等待自动回报"], ["none", 8, "结果已送达"],
] as const)("distinguishes %s notifications and delivered results (%s)", async (subscription, consumed_at, label) => {
  mockApi(`p2-notification-${subscription}-${consumed_at}`, [task({ subscription, result: { ...result, consumed_at } })]);
  mount(`p2-notification-${subscription}-${consumed_at}`);
  expect(await screen.findByText(label)).toBeTruthy();
  if (consumed_at != null) expect(((await subscriptionSelect()) as HTMLSelectElement).disabled).toBe(true);
});

it("blocks a restored draft after capability loading fails and retains dependencies and inbox on recovery", async () => {
  const parent = "p2-restored-capabilities-error";
  const { state } = mockApi(parent, [task()]);
  const first = mount(parent);
  await screen.findByRole("button", { name: "由我接管" });
  fillDraft();
  let form = screen.getByLabelText("任务名称").closest("form")!;
  fireEvent.change(within(form).getByLabelText("结果通知"), { target: { value: "inbox" } });
  fireEvent.click(within(form).getByRole("checkbox", { name: /检查恢复/ }));
  first.unmount(); first.client.clear();

  state.capabilitiesStatus = 500;
  mount(parent);
  await screen.findByRole("alert");
  fireEvent.click(screen.getByText("发起子会话"));
  form = screen.getByLabelText("任务名称").closest("form")!;
  expect(fieldValue("任务名称")).toBe("后续调查");
  expect(fieldValue("任务说明")).toBe("保留子任务草稿");
  expect((within(form).getByRole("button", { name: "开始任务" }) as HTMLButtonElement).disabled).toBe(true);
  expect(within(form).queryByText(/当前 Runtime 使用原有自动回报策略/)).toBeNull();
  expect(within(form).queryByText(/当前 Runtime 不支持任务依赖/)).toBeNull();
  await act(async () => { fireEvent.submit(form); });
  expect(state.posts).toHaveLength(0);

  state.capabilitiesStatus = 200;
  fireEvent.click(screen.getByRole("button", { name: "刷新" }));
  await waitFor(() => expect((within(form).getByRole("button", { name: "开始任务" }) as HTMLButtonElement).disabled).toBe(false));
  expect((within(form).getByLabelText("结果通知") as HTMLSelectElement).value).toBe("inbox");
  expect((within(form).getByRole("checkbox", { name: /检查恢复/ }) as HTMLInputElement).checked).toBe(true);
  fireEvent.click(within(form).getByRole("button", { name: "开始任务" }));
  await waitFor(() => expect(state.posts).toHaveLength(1));
  expect(state.posts[0]).toMatchObject({ path: `/api/sessions/${parent}/children`, body: {
    title: "后续调查", prompt: "保留子任务草稿", subscription: "inbox", depends_on: ["assignment-a"],
  } });
});

it.each(["refresh", "reset"] as const)("blocks an existing continuation after capability %s fails and preserves its chosen options", async (mode) => {
  const parent = `p2-continue-capabilities-error-${mode}`;
  const { state } = mockApi(parent, [task()]);
  const { client } = mount(parent);
  await screen.findByRole("button", { name: "由我接管" });
  fireEvent.click(screen.getByRole("button", { name: "继续任务" }));
  const form = screen.getByLabelText("后续任务").closest("form")!;
  fireEvent.change(screen.getByLabelText("后续任务"), { target: { value: "保留续做依赖与通知" } });
  fireEvent.change(within(form).getByLabelText("结果通知"), { target: { value: "inbox" } });
  fireEvent.click(within(form).getByRole("checkbox", { name: /检查恢复/ }));

  state.capabilitiesStatus = 500;
  if (mode === "reset") await client.resetQueries({ queryKey: ["child-session-capabilities", parent] });
  else fireEvent.click(screen.getByRole("button", { name: "刷新" }));
  await screen.findByRole("alert");
  expect(fieldValue("后续任务")).toBe("保留续做依赖与通知");
  expect((within(form).getByRole("button", { name: "提交后续任务" }) as HTMLButtonElement).disabled).toBe(true);
  await act(async () => { fireEvent.submit(form); });
  expect(state.posts).toHaveLength(0);

  state.capabilitiesStatus = 200;
  fireEvent.click(screen.getByRole("button", { name: "刷新" }));
  await waitFor(() => expect((within(form).getByRole("button", { name: "提交后续任务" }) as HTMLButtonElement).disabled).toBe(false));
  expect((within(form).getByLabelText("结果通知") as HTMLSelectElement).value).toBe("inbox");
  expect((within(form).getByRole("checkbox", { name: /检查恢复/ }) as HTMLInputElement).checked).toBe(true);
  fireEvent.click(within(form).getByRole("button", { name: "提交后续任务" }));
  await waitFor(() => expect(state.posts).toHaveLength(1));
  expect(state.posts[0]).toMatchObject({ path: `/api/sessions/${parent}/children/child-a/continue`, body: {
    prompt: "保留续做依赖与通知", subscription: "inbox", depends_on: ["assignment-a"],
  } });
});

it.each([404, 501])("falls back on an older host without sending P2 fields (%s)", async (status) => {
  const { state } = mockApi(`p2-legacy-${status}`, []); state.capabilitiesStatus = status;
  mount(`p2-legacy-${status}`);
  await screen.findByText(/暂无子会话任务/); fillDraft();
  await waitFor(() => expect((screen.getByRole("button", { name: "开始任务" }) as HTMLButtonElement).disabled).toBe(false));
  expect(screen.getByText(/当前 Runtime 使用原有自动回报策略/)).toBeTruthy();
  expect(screen.queryByLabelText("结果通知")).toBeNull(); expect(screen.queryByRole("checkbox")).toBeNull();
  fireEvent.click(screen.getByRole("button", { name: "开始任务" }));
  await waitFor(() => expect(state.posts).toHaveLength(1));
  expect(state.posts[0].body).not.toHaveProperty("subscription"); expect(state.posts[0].body).not.toHaveProperty("depends_on");
});

it("gates each feature by its own capability", async () => {
  const { state } = mockApi("p2-partial-caps", [task()]); state.capabilities = [CHILD_CAPABILITIES.subscriptions];
  mount("p2-partial-caps");
  await subscriptionSelect(); fillDraft();
  expect(screen.queryByRole("button", { name: "由我接管" })).toBeNull(); expect(screen.queryByRole("checkbox")).toBeNull();
  fireEvent.click(screen.getByRole("button", { name: "开始任务" }));
  await waitFor(() => expect(state.posts).toHaveLength(1));
  expect(state.posts[0].body).toHaveProperty("subscription", "auto_wake"); expect(state.posts[0].body).not.toHaveProperty("depends_on");
});

it("does not let an older cached Link restore parent controls after a task projection changes", async () => {
  const { state } = mockApi("p2-cached-control", [task()]);
  const { client } = mount("p2-cached-control");
  fireEvent.click(await screen.findByRole("button", { name: "由我接管" }));
  const dialog = await screen.findByRole("dialog");
  await waitFor(() => expect((within(dialog).getByRole("button", { name: "确认接管" }) as HTMLButtonElement).disabled).toBe(false));
  fireEvent.click(within(dialog).getByRole("button", { name: "取消" }));
  state.tasks = state.tasks.map((value) => ({ ...value, control_state: "detached", control_revision: 8 }));
  await client.invalidateQueries({ queryKey: childTasksKey("p2-cached-control") });
  await screen.findByText("已解除父子关系");
  expect(screen.queryByRole("button", { name: "由我接管" })).toBeNull();
  expect(screen.queryByRole("button", { name: "解除父子关系" })).toBeNull();
  expect((screen.getByRole("button", { name: "继续任务" }) as HTMLButtonElement).disabled).toBe(true);
});

it("continues on an old host with only the original request fields", async () => {
  const { state } = mockApi("p2-legacy-continue", [task()]); state.capabilitiesStatus = 404;
  mount("p2-legacy-continue");
  fireEvent.click(await screen.findByRole("button", { name: "继续任务" }));
  fireEvent.change(screen.getByLabelText("后续任务"), { target: { value: "兼容旧宿主的续做" } });
  await waitFor(() => expect((screen.getByRole("button", { name: "提交后续任务" }) as HTMLButtonElement).disabled).toBe(false));
  fireEvent.click(screen.getByRole("button", { name: "提交后续任务" }));
  await waitFor(() => expect(state.posts).toHaveLength(1));
  expect(Object.keys(state.posts[0].body).sort()).toEqual(["prompt", "request_id"]);
});

it("shows a detached child's parent as historical origin without implying active parent controls", async () => {
  const { state } = mockApi("p2-origin", [task({ control_state: "detached" })]);
  state.link = { ...state.link, control_state: "detached", control_revision: 7 };
  const client = new QueryClient(); const open = vi.fn();
  render(<QueryClientProvider client={client}><ChildSessionOrigin childId="child-a" parentId="p2-origin" parentTitle="原始调查" detached language="zh-CN" onOpenSession={open} /></QueryClientProvider>);
  expect(screen.getByText(/历史来源「原始调查」/)).toBeTruthy();
  expect(screen.queryByRole("button", { name: "返回父会话" })).toBeNull();
  fireEvent.click(screen.getByRole("button", { name: "查看来源会话" }));
  expect(open).toHaveBeenCalledWith("p2-origin");
  await waitFor(() => expect(screen.queryByRole("button", { name: "解除父子关系" })).toBeNull());
});
