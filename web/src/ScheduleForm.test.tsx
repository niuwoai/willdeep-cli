// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, render, screen, fireEvent, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { ScheduleForm } from "./ScheduleForm";
import { newSchedule } from "./scheduleTypes";
import { messages } from "./i18n";

let saves: Record<string, unknown>[];
let rejectFirst: boolean;
function renderForm() {
  const initial = { ...newSchedule("/workspace", "Asia/Shanghai"), name: "original", prompt: "check project" };
  const client = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } });
  const onSaved = vi.fn(); const onDirty = vi.fn();
  render(<QueryClientProvider client={client}><ScheduleForm initial={initial} id="task-id" revision={3} workspaces={[{ id: "w", path: "/workspace", name: "workspace" }]} writable language="zh-CN" messages={messages["zh-CN"]} onSaved={onSaved} onDirty={onDirty} /></QueryClientProvider>);
  return { client, onSaved, onDirty };
}
beforeEach(() => {
  saves = []; rejectFirst = false;
  vi.stubGlobal("fetch", vi.fn(async (path: string, init?: RequestInit) => {
    if (path === "/api/schedules/timezones") return new Response(JSON.stringify(["local", "Asia/Shanghai"]));
    if (path === "/api/settings/model-routing") return new Response(JSON.stringify({ providers: [], profiles: [] }));
    if (path === "/api/schedules/task-id") {
      const body = JSON.parse(String(init?.body)); saves.push(body);
      if (rejectFirst && saves.length === 1) throw new TypeError("connection lost");
      return new Response(JSON.stringify({ task: { ...body.task, id: "task-id", revision: 4 }, next_fire_at: null, last_run: null }));
    }
    return new Response("{}", { status: 404 });
  }));
});
afterEach(() => { cleanup(); vi.unstubAllGlobals(); });
describe("定时任务配置", () => {
  it("配置刷新不覆盖用户的编辑，保存带原修订号", async () => {
    const { client, onSaved } = renderForm();
    fireEvent.change(screen.getByLabelText("任务名称"), { target: { value: "edited" } });
    await client.invalidateQueries();
    expect((screen.getByLabelText("任务名称") as HTMLInputElement).value).toBe("edited");
    fireEvent.click(screen.getByRole("button", { name: "保存配置" }));
    await waitFor(() => expect(onSaved).toHaveBeenCalledOnce());
    expect(saves[0].expected_revision).toBe(3);
    expect((saves[0].task as { name: string }).name).toBe("edited");
  });
  it("保存结果不确定时保留草稿并以原请求 ID 重试", async () => {
    rejectFirst = true;
    const { onSaved } = renderForm();
    fireEvent.click(screen.getByRole("button", { name: "保存配置" }));
    await screen.findByText("保存结果尚未确认，请重试同一请求以确认结果。");
    expect(screen.getByLabelText("任务名称").matches(":disabled")).toBe(true);
    fireEvent.click(screen.getByRole("button", { name: "保存配置" }));
    await waitFor(() => expect(onSaved).toHaveBeenCalledOnce());
    expect(saves).toHaveLength(2);
    expect(saves[1]).toEqual(saves[0]);
  });
});
