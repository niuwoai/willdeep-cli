// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { ConversationSchedules } from "./ConversationSchedules";
import { messages } from "./i18n";
import { newSchedule, type ScheduleView } from "./scheduleTypes";

const NEXT_RUN_AT = 1_800_000_000;
const SCHEDULES_API = "/api/schedules";

function mount() {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } });
  render(<QueryClientProvider client={client}><ConversationSchedules sessionId="source" workspace="/ws" workspaces={[{ id: "ws", name: "ws", path: "/ws" }]} language="zh-CN" messages={messages["zh-CN"]} onOpenSession={vi.fn()} /></QueryClientProvider>);
  return userEvent.setup();
}

function trigger() {
  return screen.getByLabelText(/^会话定时任务/, { selector: "summary" });
}

function taskView(overrides: Partial<ScheduleView["task"]> = {}): ScheduleView {
  return {
    task: {
      ...newSchedule("/ws", "Asia/Shanghai"),
      id: "task", name: "当前会话复查", origin_session_id: "source", continue_in_session: true, enabled: true, revision: 2,
      created_at: NEXT_RUN_AT, updated_at: NEXT_RUN_AT, archived_at: null, completed_at: null, migration_reason: null, consecutive_failures: 0,
      ...overrides,
    },
    next_fire_at: NEXT_RUN_AT,
    last_run: null,
  };
}

function formQueryResponse(path: string) {
  if (path === `${SCHEDULES_API}/timezones`) return Response.json(["Asia/Shanghai"]);
  if (path.startsWith(`${SCHEDULES_API}/models`)) return Response.json({ models: [], warning: null });
  if (path === "/api/settings/model-routing") return Response.json({ providers: [], profiles: [] });
  return Response.json({ error: "Unexpected URL" }, { status: 404 });
}

async function openCreate(user: ReturnType<typeof userEvent.setup>) {
  await user.click(trigger());
  const create = screen.getByRole("button", { name: "新建任务" });
  await waitFor(() => expect(create.matches(":disabled")).toBe(false));
  await user.click(create);
  return screen.getByLabelText<HTMLInputElement>("任务名称");
}

afterEach(() => { cleanup(); vi.restoreAllMocks(); vi.unstubAllGlobals(); });

describe("conversation schedules", () => {
  it("opens the scoped task list from the clock and shows status, next run and history", async () => {
    const actions: unknown[] = [];
    let paused = false;
    vi.stubGlobal("fetch", vi.fn(async (path: string, init?: RequestInit) => {
      if (path.endsWith("/action")) {
        actions.push(JSON.parse(String(init?.body)));
        paused = true;
        return Response.json({});
      }
      if (path.includes("/history")) return Response.json({ runs: [], next_cursor: null });
      return Response.json({ writable: true, tasks: [taskView({ enabled: !paused }), taskView({ id: "other", name: "另一会话的任务", origin_session_id: "other" })] });
    }));
    const user = mount();
    expect(trigger().closest("details")?.open).toBe(false);
    await user.click(trigger());
    const taskButton = await screen.findByRole("button", { name: /当前会话复查/ });
    expect(trigger().getAttribute("aria-label")).toBe("会话定时任务 (1 项)");
    expect(taskButton.textContent).toContain("已启用");
    expect(taskButton.querySelector("time")?.dateTime).toBe(new Date(NEXT_RUN_AT * 1000).toISOString());
    expect(screen.queryByText("另一会话的任务")).toBeNull();
    await user.click(taskButton);
    expect(screen.queryByRole("button", { name: /当前会话复查/ })).toBeNull();
    expect(screen.getByRole("heading", { name: "当前会话复查" })).toBe(document.activeElement);
    await screen.findByText("暂无运行记录");
    await user.click(screen.getByRole("button", { name: "暂停计划" }));
    const back = screen.getByRole("button", { name: "返回列表" });
    await waitFor(() => expect(back.matches(":disabled")).toBe(false));
    expect(actions).toHaveLength(1);
    expect(actions[0]).toMatchObject({ action: "pause", expected_revision: 2 });
    expect(screen.getByLabelText("运行历史")).toBeTruthy();
    await user.click(back);
    expect(screen.getByRole("button", { name: /当前会话复查 已暂停/ })).toBeTruthy();
  });

  it("renders an archived task with an out-of-range next run", async () => {
    const archived = taskView({ archived_at: NEXT_RUN_AT, completed_at: NEXT_RUN_AT });
    vi.stubGlobal("fetch", vi.fn(async () => Response.json({
      writable: true, tasks: [{ ...archived, next_fire_at: Number.MAX_SAFE_INTEGER }],
    })));
    const user = mount();
    await waitFor(() => expect(trigger().getAttribute("aria-label")).toBe("会话定时任务 (1 项)"));
    expect(trigger().closest("details")?.open).toBe(false);
    await user.click(trigger());
    const task = screen.getByRole("button", { name: /当前会话复查 已归档/ });
    expect(task.querySelector("time")).toBeNull();
  });

  it("keeps the draft on close and discards only after confirming cancellation", async () => {
    const fetch = vi.fn(async (path: string, init?: RequestInit) => (
      path === SCHEDULES_API && !init?.method ? Response.json({ writable: true, tasks: [] }) : formQueryResponse(path)
    ));
    vi.stubGlobal("fetch", fetch);
    const user = mount();
    const name = await openCreate(user);
    await user.type(name, "未保存的任务");
    expect(screen.queryByText("暂无定时任务")).toBeNull();
    fireEvent.keyDown(name, { key: "Escape" });
    expect(document.activeElement).toBe(trigger());
    expect(trigger().closest("details")?.open).toBe(false);
    await user.click(trigger());
    expect(screen.getByLabelText<HTMLInputElement>("任务名称").value).toBe("未保存的任务");
    const confirm = vi.spyOn(window, "confirm").mockReturnValueOnce(false).mockReturnValueOnce(true);
    const cancel = screen.getByRole("button", { name: "取消" });
    await user.click(cancel);
    expect(screen.getByLabelText<HTMLInputElement>("任务名称").value).toBe("未保存的任务");
    await user.click(cancel);
    expect(confirm).toHaveBeenCalledTimes(2);
    expect(screen.queryByLabelText("任务名称")).toBeNull();
    expect(screen.getByText("暂无定时任务")).toBeTruthy();
    expect(fetch.mock.calls.some(([, init]) => init?.method === "POST")).toBe(false);
  });

  it("previews the schedule, saves it bound to the conversation and opens its detail", async () => {
    let saved: Record<string, unknown> | undefined;
    let created: ScheduleView | undefined;
    const fetch = vi.fn(async (path: string, init?: RequestInit) => {
      if (path === SCHEDULES_API && init?.method === "POST") {
        saved = JSON.parse(String(init.body));
        created = taskView({ ...(saved?.task as ScheduleView["task"]), id: "created", revision: 1 });
        return Response.json(created);
      }
      if (path === SCHEDULES_API) return Response.json({ writable: true, tasks: created ? [created] : [] });
      if (path === `${SCHEDULES_API}/preview`) return Response.json([NEXT_RUN_AT]);
      if (path.includes("/history")) return Response.json({ runs: [], next_cursor: null });
      return formQueryResponse(path);
    });
    vi.stubGlobal("fetch", fetch);
    const user = mount();
    const name = await openCreate(user);
    await user.type(name, "检查并改进");
    await user.type(screen.getByLabelText("每次运行的任务"), "读取上次结果，验证改进");
    await user.click(screen.getByRole("button", { name: "预览时间" }));
    expect((await screen.findByRole("status")).textContent).toContain("UTC");
    await user.click(screen.getByRole("button", { name: "保存配置" }));
    await screen.findByRole("heading", { name: "检查并改进" });
    expect(screen.getByRole("status").textContent).toBe("配置已保存");
    expect(screen.queryByLabelText("任务名称")).toBeNull();
    expect(saved?.task).toMatchObject({ origin_session_id: "source", continue_in_session: true, workspace: "/ws" });
    await user.click(screen.getByRole("button", { name: "返回列表" }));
    expect(screen.getByRole("button", { name: /检查并改进/ })).toBeTruthy();
  });

  it("retries a list error and shows the read-only state", async () => {
    let failed = false;
    vi.stubGlobal("fetch", vi.fn(async (path: string) => {
      if (path.includes("/history")) return Response.json({ runs: [], next_cursor: null });
      if (!failed) {
        failed = true;
        return Response.json({ error: "Connection unavailable" }, { status: 503 });
      }
      return Response.json({ writable: false, tasks: [taskView()] });
    }));
    const user = mount();
    await user.click(trigger());
    expect((await screen.findByRole("alert")).textContent).toContain("Connection unavailable");
    await user.click(screen.getByRole("button", { name: "刷新" }));
    const task = await screen.findByRole("button", { name: /当前会话复查/ });
    expect(screen.getByRole("button", { name: "新建任务" }).matches(":disabled")).toBe(true);
    expect(screen.getByRole("status").textContent).toContain("仅允许查看");
    await user.click(task);
    expect(screen.getByRole("button", { name: "暂停计划" }).matches(":disabled")).toBe(true);
  });
});
