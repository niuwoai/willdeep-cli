// @vitest-environment jsdom
import { afterEach, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { RuntimeQueue, type RuntimeQueueItem } from "./RuntimeQueue";

afterEach(() => { cleanup(); vi.unstubAllGlobals(); });
const item: RuntimeQueueItem = { id: "message-a", session_id: "session-a", text: "手机端提交的任务", text_preview: "手机端提交的任务", can_edit: true, image_count: 1, text_attachment_count: 0 };
function mount() {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } });
  render(<QueryClientProvider client={client}><RuntimeQueue sessionId="session-a" language="zh-CN" /></QueryClientProvider>);
  return client;
}
it("shows the Runtime snapshot and sends edits with the original identity and text", async () => {
  let pending = [item];
  const requests: unknown[] = [];
  vi.stubGlobal("fetch", vi.fn(async (_url: string, init?: RequestInit) => {
    if (init?.method === "PATCH") {
      requests.push(JSON.parse(String(init.body)));
      pending = [{ ...item, text: "修改后", text_preview: "修改后" }];
    }
    return { ok: true, json: async () => pending };
  }));
  mount();
  await screen.findByText(item.text);
  fireEvent.click(screen.getByRole("button", { name: "编辑" }));
  fireEvent.change(screen.getByRole("textbox"), { target: { value: "修改后" } });
  fireEvent.click(screen.getByRole("button", { name: "保存" }));
  await screen.findByText("修改后");
  expect(requests).toEqual([{ action: "edit", message_id: item.id, text: "修改后", expected_text: item.text }]);
});
it("keeps the draft visible when another client starts the message before save", async () => {
  let started = false;
  vi.stubGlobal("fetch", vi.fn(async (_url: string, init?: RequestInit) => {
    if (init?.method === "PATCH") { started = true; return { ok: false, status: 409, text: async () => "already started" }; }
    return { ok: true, json: async () => started ? [] : [item] };
  }));
  mount();
  await screen.findByText(item.text);
  fireEvent.click(screen.getByRole("button", { name: "编辑" }));
  fireEvent.change(screen.getByRole("textbox"), { target: { value: "保留此草稿" } });
  fireEvent.click(screen.getByRole("button", { name: "保存" }));
  await screen.findByRole("alert");
  await screen.findByText(/你的编辑草稿仍保留/);
  expect((screen.getByRole("textbox") as HTMLTextAreaElement).value).toBe("保留此草稿");
});
it("refreshes cross-client removals without resubmitting queued work", async () => {
  let pending = [item];
  const fetcher = vi.fn(async (_url: string, _init?: RequestInit) => ({ ok: true, json: async () => pending }));
  vi.stubGlobal("fetch", fetcher);
  const client = mount();
  await screen.findByText(item.text);
  pending = [];
  await client.invalidateQueries({ queryKey: ["runtime-queue", "session-a"] });
  await waitFor(() => expect(screen.queryByText(item.text)).toBeNull());
  expect(fetcher.mock.calls.every(([, init]) => !init?.method || init.method === "GET")).toBe(true);
});
