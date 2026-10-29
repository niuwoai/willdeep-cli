// @vitest-environment jsdom
import { afterEach, expect, it } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { ConversationCard } from "./ConversationCard";
import { messages } from "./i18n";
afterEach(cleanup);

it("keeps host details available behind a compact disclosure", () => {
  const { container } = render(<ConversationCard messages={messages["zh-CN"]} details={["original host event"]} />);
  const disclosure = container.querySelector("details")!;
  expect(disclosure.open).toBe(false);
  fireEvent.click(disclosure.querySelector("summary")!);
  expect(disclosure.open).toBe(true);
  expect(screen.getByText("original host event")).toBeTruthy();
});

it("still renders plan steps and their progress", () => {
  render(<ConversationCard messages={messages["zh-CN"]} plan={{ summary: "检查登录体验", steps: [{ id: "verify", text: "验证窄屏布局", status: "done" }] }} />);
  expect(screen.getByText("检查登录体验")).toBeTruthy();
  expect(screen.getByText("验证窄屏布局")).toBeTruthy();
});

it("shows concurrent progress and only unblocks a dependent after success", () => {
  const plan = { summary: "", steps: [
    { id: "ci", text: "Release build", status: "in_progress" as const },
    { id: "install", text: "Local install", status: "in_progress" as const },
    { id: "brew", text: "Update formula", status: "pending" as const, depends_on: ["ci"] },
  ] };
  const { rerender, container } = render(<ConversationCard messages={messages["zh-CN"]} plan={plan} />);
  expect(screen.getByText("2 项进行中")).toBeTruthy();
  expect(container.querySelectorAll('[data-status="in_progress"]').length).toBe(2);
  expect(screen.getByText("等待：Release build")).toBeTruthy();
  rerender(<ConversationCard messages={messages["zh-CN"]} plan={{ ...plan, steps: plan.steps.map((step) => step.id === "ci" ? { ...step, status: "failed" } : step) }} />);
  expect(screen.getByText("1 项进行中")).toBeTruthy();
  expect(screen.getByText("等待：Release build")).toBeTruthy();
  rerender(<ConversationCard messages={messages["en"]} plan={{ ...plan, steps: plan.steps.map((step) => step.id === "ci" ? { ...step, status: "done" } : step) }} />);
  expect(screen.getByText("1 running")).toBeTruthy();
  expect(container.querySelectorAll('[data-status="waiting"]').length).toBe(0);
  expect(screen.getByText("Pending")).toBeTruthy();
});
