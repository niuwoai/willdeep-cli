// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { ConversationSchedules } from "./ConversationSchedules";
import { messages } from "./i18n";
import { newSchedule } from "./scheduleTypes";

afterEach(() => { cleanup(); vi.unstubAllGlobals(); });
function mount() {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } });
  return render(<QueryClientProvider client={client}><ConversationSchedules sessionId="source" workspace="/ws" workspaces={[{ id: "ws", name: "ws", path: "/ws" }]} language="zh-CN" messages={messages["zh-CN"]} onOpenSession={vi.fn()} /></QueryClientProvider>);
}
describe("conversation schedules", () => {
  it("shows only the current conversation and pauses without removing history", async () => {
    const actions: unknown[] = [];
    vi.stubGlobal("fetch", vi.fn(async (path: string, init?: RequestInit) => {
      if (path.endsWith("/action")) { actions.push(JSON.parse(String(init?.body))); return Response.json({}); }
      if (path.includes("/history")) return Response.json({ runs: [], next_cursor: null });
      const task = { ...newSchedule("/ws", "Asia/Shanghai"), id: "task", name: "当前会话复查", origin_session_id: "source", continue_in_session: true, enabled: true, revision: 2 };
      return Response.json({ writable: true, tasks: [task, { ...task, id: "other", name: "另一会话的任务", origin_session_id: "other" }].map((task) => ({ task, next_fire_at: null, last_run: null })) });
    }));
    mount();
    fireEvent.click(await screen.findByRole("button", { name: /当前会话复查/ }));
    expect(screen.queryByText("另一会话的任务")).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "暂停计划" }));
    await waitFor(() => expect(actions).toHaveLength(1));
    expect(actions[0]).toMatchObject({ action: "pause", expected_revision: 2 });
    expect(screen.getByRole("button", { name: /当前会话复查/ })).toBeTruthy();
  });
  it("creates a task bound to the current session", async () => {
    let saved: Record<string, unknown> | undefined;
    vi.stubGlobal("fetch", vi.fn(async (path: string, init?: RequestInit) => {
      if (path === "/api/schedules" && init?.method === "POST") {
        saved = JSON.parse(String(init.body));
        return Response.json({ task: { ...(saved?.task as object), id: "created", revision: 1 } });
      }
      if (path.includes("timezones")) return Response.json(["Asia/Shanghai"]);
      if (path.includes("models")) return Response.json({ models: [], providers: [], profiles: [] });
      return Response.json({ writable: true, tasks: [] });
    }));
    mount();
    const create = await screen.findByRole("button", { name: "新建任务" });
    await waitFor(() => expect(create.hasAttribute("disabled")).toBe(false));
    fireEvent.click(create);
    fireEvent.change(screen.getByLabelText("任务名称"), { target: { value: "检查并改进" } });
    fireEvent.change(screen.getByLabelText("每次运行的任务"), { target: { value: "读取上次结果，验证改进" } });
    fireEvent.click(screen.getByRole("button", { name: "保存配置" }));
    await waitFor(() => expect(saved).toBeDefined());
    expect(saved?.task).toMatchObject({ origin_session_id: "source", continue_in_session: true, workspace: "/ws" });
  });
});
