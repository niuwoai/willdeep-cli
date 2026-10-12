// @vitest-environment jsdom
import { useState } from "react";
import { afterEach, describe, expect, it } from "vitest";
import { cleanup, render, screen, fireEvent } from "@testing-library/react";
import { QueuedMessages, type QueuedMessage } from "./QueuedMessages";
const initial: QueuedMessage[] = [{ id: "one", prompt: "first", attachments: [{ id: "a", kind: "text", name: "notes.txt", content: "notes" }], sessionId: "session", workspace: "/workspace", failed: false }, { id: "two", prompt: "second", attachments: [], sessionId: "session", workspace: "/workspace", failed: false }];
function Harness() { const [items, change] = useState(initial); const [open, setOpen] = useState(true); return <><QueuedMessages items={items} open={open} onOpen={setOpen} onChange={change} language="zh-CN" /><output data-testid="queue">{JSON.stringify(items)}</output></>; }
afterEach(cleanup);
describe("待发消息管理", () => {
  it("编辑期间不能恢复自动发送，取消保留原稿", () => {
    render(<Harness />); fireEvent.click(screen.getAllByRole("button", { name: "编辑" })[0]);
    fireEvent.change(screen.getByRole("textbox", { name: "编辑" }), { target: { value: "changed" } });
    expect((screen.getByRole("button", { name: "关闭管理，继续发送" }) as HTMLButtonElement).disabled).toBe(true);
    fireEvent.click(screen.getByRole("button", { name: "取消" }));
    expect(screen.getByTestId("queue").textContent).toContain('"prompt":"first"');
  });
  it("删除消息不影响另一条，删除附件不清空消息正文", () => {
    render(<Harness />); fireEvent.click(screen.getByRole("button", { name: "删除附件: notes.txt" }));
    expect(screen.getByTestId("queue").textContent).toContain('"prompt":"first","attachments":[]');
    fireEvent.click(screen.getAllByRole("button", { name: "删除" })[0]);
    expect(screen.getByTestId("queue").textContent).not.toContain('"id":"one"');
    expect(screen.getByTestId("queue").textContent).toContain('"prompt":"second"');
  });
});
