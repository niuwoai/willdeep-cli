// @vitest-environment jsdom
import { afterEach, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { SessionCollaboration } from "./SessionCollaboration";

afterEach(() => { cleanup(); vi.unstubAllGlobals(); });
function mount(sessionId: string) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(<QueryClientProvider client={client}><SessionCollaboration sessionId={sessionId} language="zh-CN" onOpenSession={vi.fn()} /></QueryClientProvider>);
}
const peer = { id: "target", title: "同伴会话", status: "idle", workspace: "/ws", last_user_request: "检查测试" };

it("keeps drafts after failed delivery and reuses the same id when retrying", async () => {
  const requests: Record<string, unknown>[] = [];
  let fail = true;
  vi.stubGlobal("fetch", vi.fn(async (path: string, init?: RequestInit) => {
    if (init?.method === "POST") {
      requests.push(JSON.parse(String(init.body)));
      return fail ? Response.json({ error: "fixture delivery not confirmed" }, { status: 503 }) : Response.json({});
    }
    return Response.json(path.endsWith("/peers") ? [peer] : []);
  }));
  mount("source-retry");
  fireEvent.click(screen.getAllByText("会话协作")[0]);
  await screen.findByText("同伴会话");
  fireEvent.change(screen.getByLabelText("接收会话"), { target: { value: "target" } });
  fireEvent.change(screen.getByLabelText("协作消息"), { target: { value: "请保留提交，不要重复发布" } });
  fireEvent.click(screen.getByRole("button", { name: "发送" }));
  await screen.findByRole("alert");
  expect(screen.getByRole("alert").textContent).toContain("fixture delivery not confirmed");
  expect((screen.getByLabelText("协作消息") as HTMLTextAreaElement).value).toContain("不要重复发布");
  fail = false;
  fireEvent.click(screen.getByRole("button", { name: "发送" }));
  await waitFor(() => expect(requests.length).toBe(2));
  expect(requests[0].request_id).toBe(requests[1].request_id);
  expect(requests[0]).toMatchObject({ session_id: "target", message: "请保留提交，不要重复发布", reply_to: null });
  await waitFor(() => expect((screen.getByLabelText("协作消息") as HTMLTextAreaElement).value).toBe(""));
});

it("distinguishes queuing from model consumption and retains drafts when replying", async () => {
  const note = { id: "note", from_session: "target", to_session: "source-display", body: "已完成测试，等待集成", reply_to: null, consumed_at: null, read_by: [] };
  vi.stubGlobal("fetch", vi.fn(async (path: string) => Response.json(path.endsWith("/peers") ? [peer] : [note])));
  mount("source-display");
  fireEvent.click(screen.getAllByText("会话协作")[0]);
  await screen.findByText("已完成测试，等待集成");
  expect(screen.getByText("已入队")).toBeTruthy();
  expect(screen.queryByText("已送入模型上下文")).toBeNull();
  fireEvent.click(screen.getByRole("button", { name: "回复" }));
  fireEvent.change(screen.getByLabelText("协作消息"), { target: { value: "我来集成" } });
  fireEvent.click(screen.getByRole("button", { name: "取消回复关联" }));
  expect((screen.getByLabelText("协作消息") as HTMLTextAreaElement).value).toBe("我来集成");
  expect(screen.getByText(/不能替代用户授权/)).toBeTruthy();
});

it("dismisses with Escape and outside clicks without losing the coordination draft", async () => {
  vi.stubGlobal("fetch", vi.fn(async (path: string) => Response.json(path.endsWith("/peers") ? [peer] : [])));
  const { container } = mount("source-dismiss");
  const disclosure = container.querySelector("details")!;
  const trigger = disclosure.querySelector("summary")!;
  expect(disclosure.open).toBe(false);
  fireEvent.click(trigger);
  await screen.findByText("同伴会话");
  fireEvent.change(screen.getByLabelText("接收会话"), { target: { value: "target" } });
  fireEvent.change(screen.getByLabelText("协作消息"), { target: { value: "保留这份交接草稿" } });
  fireEvent.keyDown(document, { key: "Escape" });
  expect(disclosure.open).toBe(false);
  expect(document.activeElement).toBe(trigger);
  fireEvent.click(trigger);
  expect((screen.getByLabelText("协作消息") as HTMLTextAreaElement).value).toBe("保留这份交接草稿");
  fireEvent.pointerDown(document.body);
  expect(disclosure.open).toBe(false);
});
