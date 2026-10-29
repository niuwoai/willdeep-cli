// @vitest-environment jsdom
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { UserMessage } from "./UserMessage";
import { messages } from "./i18n";

let height = 400;
let resize: () => void;
beforeEach(() => {
  height = 400;
  vi.spyOn(HTMLElement.prototype, "scrollHeight", "get").mockImplementation(() => height);
  vi.spyOn(window, "getComputedStyle").mockReturnValue({ lineHeight: "20px", getPropertyValue: () => "12" } as unknown as CSSStyleDeclaration);
  vi.stubGlobal("ResizeObserver", class {
    constructor(callback: () => void) { resize = callback; }
    observe() {}
    disconnect() {}
  });
});
afterEach(() => { cleanup(); vi.restoreAllMocks(); vi.unstubAllGlobals(); });

it("folds long messages, expands and folds again without changing the original text", () => {
  const content = "长消息和代码\n".repeat(30) + "完整末尾";
  const { container } = render(<UserMessage content={content} messages={messages["zh-CN"]} />);
  const toggle = screen.getByRole("button", { name: "显示更多" });
  const body = container.querySelector(".user-message-body")!;
  expect(body.textContent).toBe(content);
  expect(toggle.getAttribute("aria-expanded")).toBe("false");
  expect(toggle.getAttribute("aria-controls")).toBe(body.id);
  fireEvent.click(toggle);
  expect(screen.getByRole("button", { name: "收起" }).getAttribute("aria-expanded")).toBe("true");
  fireEvent.click(toggle);
  expect(container.querySelector('[data-collapsed="true"]')).toBeTruthy();
  expect(body.textContent).toBe(content);
});

it("uses measured layout so short messages and exactly twelve lines have no toggle", () => {
  height = 240;
  render(<UserMessage content="short" messages={messages.en} />);
  expect(screen.queryByRole("button")).toBeNull();
});

it("rechecks wrapping after resize and preserves per-message expansion across updates", async () => {
  const { act } = await import("@testing-library/react");
  height = 100;
  const { rerender } = render(<UserMessage content="single paragraph" messages={messages.en} />);
  expect(screen.queryByRole("button")).toBeNull();
  height = 400;
  act(() => resize());
  fireEvent.click(screen.getByRole("button", { name: "Show more" }));
  rerender(<UserMessage content="single paragraph" messages={messages.ja} />);
  expect(screen.getByRole("button", { name: "折りたたむ" }).getAttribute("aria-expanded")).toBe("true");
  height = 100;
  act(() => resize());
  expect(screen.queryByRole("button")).toBeNull();
});

it("keeps duplicate messages independently expandable", () => {
  render(<><UserMessage content="same" messages={messages.en} /><UserMessage content="same" messages={messages.en} /></>);
  fireEvent.click(screen.getAllByRole("button", { name: "Show more" })[0]);
  expect(screen.getAllByRole("button", { name: "Show less" })).toHaveLength(1);
  expect(screen.getAllByRole("button", { name: "Show more" })).toHaveLength(1);
});
