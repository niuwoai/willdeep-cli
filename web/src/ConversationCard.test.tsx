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
