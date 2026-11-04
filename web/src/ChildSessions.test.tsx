// @vitest-environment jsdom
import { afterEach, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { ChildSessionOrigin, ChildSessions } from "./ChildSessions";
import type { ChildTask } from "./childSessionsApi";

afterEach(() => { cleanup(); vi.unstubAllGlobals(); });
const task = (overrides: Partial<ChildTask> = {}): ChildTask => ({
  assignment_id: "assignment", parent_session_id: "parent", child_session_id: "child", child_turn_id: "turn",
  title: "检查会话恢复", prompt: "调查恢复流程，交回结论", status: "running",
  context: "fresh", workspace_mode: "shared_read_only", model: null, through_turn_id: null,
  created_at: 1, updated_at: 1, result: null, error: null, ...overrides,
});
function mount(id: string, onOpenSession = vi.fn()) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } });
  return { onOpenSession, ...render(<QueryClientProvider client={client}>
    <textarea aria-label="父会话草稿" defaultValue="等调查结果再修改" />
    <ChildSessions sessionId={id} language="zh-CN" onOpenSession={onOpenSession} />
  </QueryClientProvider>) };
}

it("preserves both drafts and the request key on failed creation, without opening a child or stealing focus", async () => {
  const requests: Record<string, unknown>[] = [];
  let fail = true;
  vi.stubGlobal("fetch", vi.fn(async (_path: string, init?: RequestInit) => {
    if (init?.method === "POST") {
      requests.push(JSON.parse(String(init.body)) as Record<string, unknown>);
      return fail ? Response.json({ error: "response lost" }, { status: 503 }) : Response.json(task());
    }
    return Response.json({ tasks: [], next_cursor: null });
  }));
  const { onOpenSession } = mount("parent-create");
  await screen.findByText(/暂无子会话任务/);
  fireEvent.click(screen.getByText("发起子会话"));
  fireEvent.change(screen.getByLabelText("任务名称"), { target: { value: "恢复调查" } });
  fireEvent.change(screen.getByLabelText("任务说明"), { target: { value: "只读调查并报告风险" } });
  fireEvent.click(screen.getByRole("button", { name: "开始任务" }));
  await screen.findByRole("alert");
  expect((screen.getByLabelText("任务说明") as HTMLTextAreaElement).value).toBe("只读调查并报告风险");
  expect((screen.getByLabelText("任务名称") as HTMLInputElement).value).toBe("恢复调查");
  fail = false;
  fireEvent.click(screen.getByRole("button", { name: "开始任务" }));
  screen.getByLabelText("父会话草稿").focus();
  await screen.findByText("子会话任务已创建，你可以继续当前对话。");
  expect(requests).toHaveLength(2);
  expect(requests[0].request_id).toBe(requests[1].request_id);
  expect(requests[0]).toMatchObject({ title: "恢复调查", prompt: "只读调查并报告风险", context: "fresh", workspace_mode: "shared_read_only" });
  expect(onOpenSession).not.toHaveBeenCalled();
  expect((screen.getByLabelText("父会话草稿") as HTMLTextAreaElement).value).toBe("等调查结果再修改");
  expect(document.activeElement).toBe(screen.getByLabelText("父会话草稿"));
});

it("shows waiting as active and opens the real child conversation", async () => {
  vi.stubGlobal("fetch", vi.fn(async () => Response.json({ tasks: [task({ status: "waiting_answer" })], next_cursor: null })));
  const { onOpenSession } = mount("parent-waiting");
  await screen.findByText("等待回答");
  expect(screen.queryByText("本次任务已结束")).toBeNull();
  expect(screen.getByRole("button", { name: "停止本次任务" })).toBeTruthy();
  expect(screen.queryByRole("button", { name: "重试任务" })).toBeNull();
  fireEvent.click(screen.getByRole("button", { name: "打开完整对话" }));
  expect(onOpenSession).toHaveBeenCalledWith("child");
});

it("keeps a terminal assignment distinct from its conversation and sends guarded retry revisions", async () => {
  const requests: { path: string; body: Record<string, unknown> }[] = [];
  const failed = task({ status: "interrupted", result: { revision: 7, summary: "", report: null, report_complete: false, consumed_at: null, superseded_by: null } });
  vi.stubGlobal("fetch", vi.fn(async (path: string, init?: RequestInit) => {
    if (init?.method === "POST") { requests.push({ path, body: JSON.parse(String(init.body)) }); return Response.json(task()); }
    return Response.json({ tasks: [failed], next_cursor: null });
  }));
  mount("parent-retry");
  await screen.findByText("本次任务已中断");
  expect(screen.getByText("结果待处理")).toBeTruthy();
  fireEvent.click(screen.getByText("本次任务结果"));
  expect(screen.getByText("本次任务没有可用报告。")).toBeTruthy();
  expect(screen.getByText(/报告可能不完整/)).toBeTruthy();
  fireEvent.click(screen.getByRole("button", { name: "重试任务" }));
  await waitFor(() => expect(requests.length).toBe(1));
  expect(requests[0].path).toBe("/api/sessions/parent-retry/children/assignment/retry");
  expect(requests[0].body).toMatchObject({ expected_result_revision: 7 });
  expect(requests[0].body.request_id).toBeTypeOf("string");
});

it("continues the same child with a new prompt and stops only the selected assignment", async () => {
  const requests: { path: string; body: Record<string, unknown> }[] = [];
  let current = task({ status: "cancelled" });
  vi.stubGlobal("fetch", vi.fn(async (path: string, init?: RequestInit) => {
    if (init?.method === "POST") {
      requests.push({ path, body: JSON.parse(String(init.body)) });
      current = task({ status: "running" });
      return Response.json(current);
    }
    return Response.json({ tasks: [current], next_cursor: null });
  }));
  mount("parent-continue");
  await screen.findByText("本次任务已取消");
  expect(screen.queryByRole("button", { name: "重试任务" })).toBeNull();
  fireEvent.click(screen.getByRole("button", { name: "继续任务" }));
  fireEvent.change(screen.getByLabelText("后续任务"), { target: { value: "继续检查边界" } });
  fireEvent.click(screen.getByRole("button", { name: "提交后续任务" }));
  await screen.findByText("运行中");
  expect(requests[0]).toMatchObject({ path: "/api/sessions/parent-continue/children/child/continue", body: { prompt: "继续检查边界" } });
  fireEvent.click(screen.getByRole("button", { name: "停止本次任务" }));
  await waitFor(() => expect(requests.length).toBe(2));
  expect(requests[1].path).toBe("/api/sessions/parent-continue/children/assignment/stop");
  expect(Object.keys(requests[1].body)).toEqual(["request_id"]);
});

it("sends inherited-history, worktree and model options without opening the child", async () => {
  const requests: Record<string, unknown>[] = [];
  vi.stubGlobal("fetch", vi.fn(async (_path: string, init?: RequestInit) => {
    if (init?.method === "POST") {
      requests.push(JSON.parse(String(init.body)) as Record<string, unknown>);
      return Response.json(task({ context: "fork", workspace_mode: "worktree", model: "worker-model" }));
    }
    return Response.json({ tasks: [], next_cursor: null });
  }));
  const { onOpenSession } = mount("parent-options");
  await screen.findByText(/暂无子会话任务/);
  fireEvent.click(screen.getByText("发起子会话"));
  fireEvent.change(screen.getByLabelText("任务名称"), { target: { value: "继承后继续" } });
  fireEvent.change(screen.getByLabelText("任务说明"), { target: { value: "在独立工作树里实现并验证" } });
  fireEvent.change(screen.getByLabelText("上下文"), { target: { value: "fork" } });
  fireEvent.change(screen.getByLabelText("完成轮次 ID（留空取最近一次）"), { target: { value: "through-id" } });
  fireEvent.change(screen.getByLabelText("工作区"), { target: { value: "worktree" } });
  fireEvent.change(screen.getByLabelText("工作树基线"), { target: { value: "snapshot" } });
  fireEvent.change(screen.getByLabelText("模型（可选）"), { target: { value: "worker-model" } });
  fireEvent.click(screen.getByRole("button", { name: "开始任务" }));
  await waitFor(() => expect(requests.length).toBe(1));
  expect(requests[0]).toMatchObject({
    context: "fork", through_turn_id: "through-id",
    workspace_mode: "worktree", worktree_base: "snapshot", model: "worker-model",
  });
  expect(onOpenSession).not.toHaveBeenCalled();
});

it.each([[501, "当前 Runtime 不支持子会话"], [403, "此入口仅限本机访问"]])("explains unavailable Runtime or private permissions (%s)", async (status, text) => {
  vi.stubGlobal("fetch", vi.fn(async () => Response.json({ error: "private detail" }, { status })));
  mount(`parent-unavailable-${status}`);
  expect((await screen.findByRole("alert")).textContent).toContain(text);
  expect(screen.queryByText("发起子会话")).toBeNull();
  expect(screen.queryByText("private detail")).toBeNull();
});

it("keeps paginated assignments accessible and returns from a child to its parent", async () => {
  const fetcher = vi.fn(async (path: string) => Response.json(path.includes("after=cursor")
    ? { tasks: [task({ assignment_id: "second", title: "第二项调查" })], next_cursor: null }
    : { tasks: [task()], next_cursor: "cursor" }));
  vi.stubGlobal("fetch", fetcher);
  const { unmount } = mount("parent-pages");
  fireEvent.click(await screen.findByRole("button", { name: "加载更多任务" }));
  await screen.findByText("第二项调查");
  expect(screen.getAllByRole("button", { name: "打开完整对话" })).toHaveLength(2);
  unmount();
  const open = vi.fn();
  render(<ChildSessionOrigin parentId="parent" parentTitle="主调查" language="zh-CN" onOpenSession={open} />);
  expect(screen.getByText(/来自「主调查」/)).toBeTruthy();
  fireEvent.click(screen.getByRole("button", { name: "返回父会话" }));
  expect(open).toHaveBeenCalledWith("parent");
});
