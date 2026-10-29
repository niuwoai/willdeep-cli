// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import { RuntimeHumanGates } from "./RuntimeHumanGates";
import { messages } from "./i18n";

afterEach(cleanup);

it("renders four full-row choices and sends the exact selected option only once", async () => {
  const answer = vi.fn(async () => {});
  const options = ["A: 第一种方案", "B: 第二种方案", "C: 第三种方案", "D: 第四种方案"];
  const gate = { id: "choice", task_id: "task", kind: "question" as const, question: "请选择方案", options, multi_select: false };
  const { container } = render(<RuntimeHumanGates gates={[gate]} messages={messages["zh-CN"]} language="zh-CN" onAnswer={answer} onApproval={vi.fn()} />);
  expect(container.querySelectorAll(".human-gate-option")).toHaveLength(4);
  expect(container.querySelector("details")).toBeNull();
  fireEvent.click(screen.getByRole("button", { name: options[2] }));
  fireEvent.click(screen.getByRole("button", { name: options[1] }));
  await screen.findByRole("status");
  expect(answer).toHaveBeenCalledExactlyOnceWith("choice", options[2]);
});

it("keeps a custom answer on failure and allows retry through the form", async () => {
  const answer = vi.fn().mockRejectedValueOnce(new Error("offline")).mockResolvedValue(undefined);
  const gate = { id: "custom", task_id: "task", kind: "question" as const, question: "自定义方案", options: [], multi_select: false };
  render(<RuntimeHumanGates gates={[gate]} messages={messages.en} language="en" onAnswer={answer} onApproval={vi.fn()} />);
  const input = screen.getByRole("textbox");
  fireEvent.change(input, { target: { value: "  My alternative  " } });
  fireEvent.submit(input.closest("form")!);
  await screen.findByRole("alert");
  expect((input as HTMLInputElement).value).toBe("  My alternative  ");
  fireEvent.submit(input.closest("form")!);
  await screen.findByRole("status");
  expect(answer).toHaveBeenLastCalledWith("custom", "My alternative");
});

it("multi-select waits for explicit submission and preserves all chosen options", async () => {
  const answer = vi.fn(async () => {});
  const gate = { id: "multi", task_id: "task", kind: "question" as const, question: "选择多项", options: ["A", "B", "C"], multi_select: true };
  render(<RuntimeHumanGates gates={[gate]} messages={messages.en} language="en" onAnswer={answer} onApproval={vi.fn()} />);
  fireEvent.click(screen.getByRole("checkbox", { name: "A" }));
  fireEvent.click(screen.getByRole("checkbox", { name: "C" }));
  expect(answer).not.toHaveBeenCalled();
  fireEvent.click(screen.getAllByRole("button", { name: messages.en.submitAnswer })[0]);
  await waitFor(() => expect(answer).toHaveBeenCalledWith("multi", "A, C"));
});
it("retains the draft and request identity across parking and restoration", () => {
  const answer = vi.fn(async () => {});
  const gate = { id: "old-question", task_id: "old-task", kind: "question" as const, question: "原问题", options: [], multi_select: false, parked: false };
  const props = { messages: messages["zh-CN"], language: "zh-CN" as const, onAnswer: answer, onApproval: vi.fn(async () => {}) };
  const view = render(<RuntimeHumanGates {...props} gates={[gate]} />);
  fireEvent.change(screen.getByRole("textbox"), { target: { value: "原问题的回答" } });
  view.rerender(<RuntimeHumanGates {...props} gates={[{ ...gate, parked: true }]} />);
  view.rerender(<RuntimeHumanGates {...props} gates={[gate]} />);
  expect((screen.getByRole("textbox") as HTMLInputElement).value).toBe("原问题的回答");
  fireEvent.click(screen.getByRole("button", { name: props.messages.submitAnswer }));
  expect(answer).toHaveBeenCalledWith("old-question", "原问题的回答");
});

it("parking never submits an approval and each card keeps its own target", () => {
  const approval = vi.fn(async () => {});
  const gates = ["original", "independent"].map((id) => ({ id, task_id: id, kind: "approval" as const, description: id, always_allow_available: false, parked: true }));
  const t = messages["zh-CN"];
  render(<RuntimeHumanGates gates={gates} messages={t} language="zh-CN" onAnswer={vi.fn(async () => {})} onApproval={approval} />);
  expect(approval).not.toHaveBeenCalled();
  fireEvent.click(screen.getAllByRole("button", { name: t.allowOnce })[1]);
  expect(approval).toHaveBeenCalledWith("independent", "allow_once");
  const actions = screen.getAllByRole("button", { name: t.deny })[0].parentElement!;
  expect(actions.className).toBe("gate-actions");
  expect(actions.querySelector(".gate-action.primary")?.textContent).toBe(t.allowOnce);
  expect(actions.querySelector(".gate-action.secondary")?.textContent).toBe(t.deny);
});
