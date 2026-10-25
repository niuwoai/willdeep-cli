// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import { RuntimeHumanGates } from "./RuntimeHumanGates";
import { messages } from "./i18n";

afterEach(cleanup);
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
  fireEvent.click(screen.getAllByText(t.allowOnce)[1]);
  expect(approval).toHaveBeenCalledWith("independent", "allow_once");
});
