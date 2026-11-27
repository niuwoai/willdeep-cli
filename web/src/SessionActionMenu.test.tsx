// @vitest-environment jsdom
import { ChakraProvider, defaultSystem } from "@chakra-ui/react";
import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { SessionActionMenu } from "./SessionActionMenu";
import { messages } from "./i18n";

const t = messages.en;
const title = "Example session";

beforeEach(() => {
  // Provide layout inputs for Floating UI: jsdom otherwise reports a zero-sized viewport and anchor.
  vi.spyOn(HTMLElement.prototype, "offsetWidth", "get").mockReturnValue(28);
  vi.spyOn(Element.prototype, "clientWidth", "get").mockReturnValue(1024);
  vi.spyOn(Element.prototype, "clientHeight", "get").mockReturnValue(768);
  vi.spyOn(Element.prototype, "getBoundingClientRect").mockReturnValue(new DOMRect(40, 70, 28, 26));
  vi.stubGlobal("ResizeObserver", class { observe() {} unobserve() {} disconnect() {} });
  vi.stubGlobal("IntersectionObserver", class { observe() {} unobserve() {} disconnect() {} });
  vi.stubGlobal("CSS", { escape: (value: string) => value.replace(/[^\w-]/g, "\\$&") });
});
afterEach(() => { cleanup(); vi.restoreAllMocks(); vi.unstubAllGlobals(); });

function mount({ active = false, busy = false } = {}) {
  const user = userEvent.setup();
  const onAction = vi.fn();
  render(
    <ChakraProvider value={defaultSystem}>
      <div className="session-list">
        <div className="session-row">
          <button type="button">{title}</button>
          <SessionActionMenu title={title} messages={t} pinned={false} archived={false} busy={busy} active={active} forkDisabled={false} onAction={onAction} />
        </div>
      </div>
      <input aria-label="Search" />
    </ChakraProvider>,
  );
  return { user, onAction, trigger: screen.getByRole("button", { name: t.sessionActionsFor.replace("{title}", title) }) };
}

async function focusedMenu() {
  const menu = await screen.findByRole("menu");
  await waitFor(() => expect(document.activeElement).toBe(menu));
  return menu;
}

it("opens by keyboard, highlights menu items and restores focus on Escape", async () => {
  const { user, trigger, onAction } = mount();
  await user.tab();
  await user.tab();
  expect(document.activeElement).toBe(trigger);
  await user.keyboard("{ArrowDown}");
  const menu = await focusedMenu();
  expect(menu.closest(".session-list")).toBeNull();
  expect(menu.parentElement?.style.position).toBe("fixed");
  const pinItem = within(menu).getByRole("menuitem", { name: t.pinSession });
  const renameItem = within(menu).getByRole("menuitem", { name: t.renameSession });
  await waitFor(() => expect(menu.getAttribute("aria-activedescendant")).toBe(pinItem.id));
  await user.keyboard("{ArrowDown}");
  expect(menu.getAttribute("aria-activedescendant")).toBe(renameItem.id);
  await user.keyboard("{ArrowUp}");
  expect(menu.getAttribute("aria-activedescendant")).toBe(pinItem.id);
  await user.keyboard("{Escape}");
  expect(trigger.getAttribute("aria-expanded")).toBe("false");
  await waitFor(() => expect(document.activeElement).toBe(trigger));
  expect(onAction).not.toHaveBeenCalled();
  await user.keyboard("{Enter}");
  await focusedMenu();
  await user.keyboard("{Enter}");
  expect(onAction.mock.calls).toEqual([["pin"]]);
  expect(document.activeElement).toBe(trigger);
});

it.each([["{Tab}", "Search"], ["{Shift>}{Tab}{/Shift}", title]])("leaves the menu with %s", async (keys, nextName) => {
  const { user, trigger } = mount();
  await user.click(trigger);
  await focusedMenu();
  await user.keyboard(keys);
  await waitFor(() => expect(trigger.getAttribute("aria-expanded")).toBe("false"));
  const next = screen.getByRole(nextName === "Search" ? "textbox" : "button", { name: nextName });
  expect(document.activeElement).toBe(next);
});

it.each([["p", "pin"], ["r", "rename"], ["f", "fork"], ["a", "archive"], ["d", "delete"]])(
  "runs %s only while menu focus is active",
  async (key, action) => {
    const { user, trigger, onAction } = mount();
    const search = screen.getByRole<HTMLInputElement>("textbox", { name: "Search" });
    await user.type(search, key);
    expect(search.value).toBe(key);
    expect(onAction).not.toHaveBeenCalled();
    await user.click(trigger);
    const menu = await focusedMenu();
    fireEvent.keyDown(menu, { key, isComposing: true });
    fireEvent.keyDown(menu, { key, repeat: true });
    fireEvent.keyDown(menu, { key, metaKey: true });
    expect(onAction).not.toHaveBeenCalled();
    await user.keyboard(key);
    expect(onAction.mock.calls).toEqual([[action]]);
    expect(trigger.getAttribute("aria-expanded")).toBe("false");
    expect(document.activeElement).toBe(trigger);
  },
);

it.each([false, true])("does not activate disabled actions (locally busy: %s)", async (busy) => {
  const { user, trigger, onAction } = mount({ active: true, busy });
  await user.click(trigger);
  const menu = await focusedMenu();
  const rename = within(menu).getByRole("menuitem", { name: t.renameSession });
  expect(rename.getAttribute("aria-disabled")).toBe("true");
  fireEvent.click(rename);
  await user.keyboard("rfad");
  expect(onAction).not.toHaveBeenCalled();
  const search = screen.getByRole<HTMLInputElement>("textbox", { name: "Search" });
  await user.click(search);
  await waitFor(() => expect(trigger.getAttribute("aria-expanded")).toBe("false"));
  expect(document.activeElement).toBe(search);
  await user.keyboard("rfad");
  expect(search.value).toBe("rfad");
});
