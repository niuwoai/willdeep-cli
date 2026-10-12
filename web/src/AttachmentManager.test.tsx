// @vitest-environment jsdom
import { useState } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, render, screen, fireEvent } from "@testing-library/react";
import { AttachmentManager } from "./AttachmentManager";
import type { DraftAttachment } from "./attachmentModel";

const fixtures: DraftAttachment[] = [
  { id: "first", kind: "text", name: "same.txt", content: "first content" },
  { id: "second", kind: "text", name: "same.txt", content: "second content" },
];
function Harness({ readOnly = false }: { readOnly?: boolean }) {
  const [items, setItems] = useState(fixtures);
  return <><AttachmentManager items={items} onChange={setItems} language="zh-CN" readOnly={readOnly} /><output data-testid="contents">{items.map((item) => item.kind === "text" ? `${item.id}:${item.content}` : item.id).join("|")}</output></>;
}
beforeEach(() => {
  HTMLDialogElement.prototype.showModal = function () { this.open = true; };
  HTMLDialogElement.prototype.close = function () { this.open = false; };
});
afterEach(() => { cleanup(); vi.restoreAllMocks(); });
describe("附件管理", () => {
  it("重名附件按稳定 ID 删除与移动", () => {
    render(<Harness />);
    fireEvent.click(screen.getAllByLabelText("附件排序: same.txt")[0]);
    fireEvent.click(screen.getAllByRole("button", { name: "下移: same.txt" })[0]);
    expect(screen.getByTestId("contents").textContent).toBe("second:second content|first:first content");
    fireEvent.click(screen.getAllByRole("button", { name: "删除附件: same.txt" })[0]);
    expect(screen.getByTestId("contents").textContent).toBe("first:first content");
  });
  it("文本编辑取消保留原稿，保存只更新选中的附件", () => {
    render(<Harness />);
    fireEvent.click(screen.getAllByRole("button", { name: "编辑文本" })[1]);
    fireEvent.change(screen.getByRole("textbox", { name: "编辑文本" }), { target: { value: "changed" } });
    fireEvent.click(screen.getByRole("button", { name: "取消编辑" }));
    expect(screen.getByTestId("contents").textContent).toContain("second:second content");
    fireEvent.click(screen.getAllByRole("button", { name: "编辑文本" })[1]);
    fireEvent.change(screen.getByRole("textbox", { name: "编辑文本" }), { target: { value: "saved" } });
    fireEvent.click(screen.getByRole("button", { name: "保存文本" }));
    expect(screen.getByTestId("contents").textContent).toBe("first:first content|second:saved");
  });
  it("只读附件可预览但没有编辑删除入口", () => {
    render(<Harness readOnly />);
    expect(screen.queryByRole("button", { name: "编辑文本" })).toBeNull();
    expect(screen.queryByRole("button", { name: "删除附件: same.txt" })).toBeNull();
    fireEvent.click(screen.getAllByTitle("预览附件")[0]);
    expect(screen.getByText("first content")).toBeTruthy();
  });
});
