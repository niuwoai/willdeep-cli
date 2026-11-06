// @vitest-environment jsdom
import { ChakraProvider, defaultSystem } from "@chakra-ui/react";
import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import { WorkspaceApprovalGuide } from "./WorkspaceApprovalGuide";
import { ApprovalModeFailure } from "./approvalModeApi";
import { messages, type Language } from "./i18n";
import { workspaceApprovalText } from "./workspaceApprovalMessages";

afterEach(() => { cleanup(); vi.restoreAllMocks(); vi.unstubAllGlobals(); });
function mount(language: Language = "zh-CN", save = vi.fn(async () => {})) {
  const cancel = vi.fn();
  const view = render(<ChakraProvider value={defaultSystem}><WorkspaceApprovalGuide path="/actual/directory" language={language} messages={messages[language]} onSave={save} onCancel={cancel} /></ChakraProvider>);
  return { ...view, save, cancel, t: workspaceApprovalText(language) };
}

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason?: unknown) => void;
  const promise = new Promise<T>((resolvePromise, rejectPromise) => { resolve = resolvePromise; reject = rejectPromise; });
  return { promise, resolve, reject };
}

type DismissAction = "cancel" | "escape" | "backdrop";
async function waitForDialogDismissHandlers() {
  await new Promise<void>((resolve) => requestAnimationFrame(() => resolve()));
  await new Promise<void>((resolve) => setTimeout(resolve, 0));
}
function dismiss(action: DismissAction, t: ReturnType<typeof workspaceApprovalText>) {
  if (action === "cancel") fireEvent.click(screen.getByRole("button", { name: messages["zh-CN"].cancel }));
  else if (action === "escape") fireEvent.keyDown(screen.getByRole("dialog", { name: t.title }), { key: "Escape" });
  else {
    const backdrop = document.querySelector('[data-scope="dialog"][data-part="backdrop"]');
    if (!backdrop) throw new Error(`Dialog backdrop missing for ${t.title}`);
    fireEvent.pointerDown(backdrop);
  }
}

it("defaults and focuses Smart, shows the actual path, and saves Smart", async () => {
  // jsdom 没有布局；焦点陷阱需看到默认选项的实际矩形才会认为它可聚焦。
  vi.spyOn(HTMLInputElement.prototype, "getClientRects").mockImplementation(function (this: HTMLInputElement) {
    return { length: this.value === "smart" ? 1 : 0 } as DOMRectList;
  });
  const { save, t } = mount();
  const smart = screen.getByRole("radio", { name: /^智能审核/ }) as HTMLInputElement;
  expect(smart.checked).toBe(true);
  await waitFor(() => expect(document.activeElement).toBe(smart));
  expect(screen.getByText("/actual/directory")).toBeTruthy();
  expect(screen.getAllByRole("radio")).toHaveLength(3);
  fireEvent.click(screen.getByRole("button", { name: t.continue }));
  await waitFor(() => expect(save).toHaveBeenCalledWith("smart", false));
});

it("cancel never saves the selected mode", () => {
  const { save, cancel } = mount();
  fireEvent.click(screen.getByRole("radio", { name: /^严格模式/ }));
  fireEvent.click(screen.getByRole("button", { name: messages["zh-CN"].cancel }));
  expect(cancel).toHaveBeenCalledOnce();
  expect(save).not.toHaveBeenCalled();
});

it.each(
  (["cancel", "escape", "backdrop"] as const).flatMap((action) =>
    (["success", "failure"] as const).map((outcome) => [action, outcome] as const),
  ),
)("blocks %s dismissal during a deferred %s save", async (action, outcome) => {
  const pending = deferred<void>();
  const save = vi.fn(() => pending.promise);
  const { cancel, t } = mount("zh-CN", save);
  fireEvent.click(screen.getByRole("button", { name: t.continue }));
  await waitFor(() => expect(save).toHaveBeenCalledOnce());
  await waitForDialogDismissHandlers();

  const cancelButton = screen.getByRole("button", { name: messages["zh-CN"].cancel }) as HTMLButtonElement;
  expect(cancelButton.disabled).toBe(true);
  dismiss(action, t);
  await waitForDialogDismissHandlers();
  expect(cancel).not.toHaveBeenCalled();
  expect(screen.getByRole("dialog", { name: t.title })).toBeTruthy();

  if (outcome === "success") {
    await act(async () => {
      pending.resolve();
      await pending.promise;
    });
    expect(save).toHaveBeenCalledOnce();
    expect(cancel).not.toHaveBeenCalled();
  } else {
    pending.reject(new Error("disk unavailable"));
    expect((await screen.findByRole("alert")).textContent).toContain("disk unavailable");
    expect(cancelButton.disabled).toBe(false);
    dismiss(action, t);
    await waitForDialogDismissHandlers();
    expect(cancel).toHaveBeenCalledOnce();
  }
});

it("requires the existing full access acknowledgement and clears it when the choice changes", async () => {
  const { save, t } = mount();
  const apply = screen.getByRole("button", { name: t.continue }) as HTMLButtonElement;
  fireEvent.click(screen.getByRole("radio", { name: /^完全访问/ }));
  expect(apply.disabled).toBe(true);
  fireEvent.click(apply);
  expect(save).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole("checkbox", { name: /我知道/ }));
  expect(apply.disabled).toBe(false);
  fireEvent.click(screen.getByRole("radio", { name: /^智能审核/ }));
  fireEvent.click(screen.getByRole("radio", { name: /^完全访问/ }));
  expect(apply.disabled).toBe(true);
  fireEvent.click(screen.getByRole("checkbox", { name: /我知道/ }));
  fireEvent.click(apply);
  await waitFor(() => expect(save).toHaveBeenCalledWith("full_access", true));
});

it("keeps the selection and allows retry after a save error", async () => {
  const save = vi.fn().mockRejectedValueOnce(new Error("disk unavailable")).mockResolvedValueOnce(undefined);
  const { t } = mount("zh-CN", save);
  fireEvent.click(screen.getByRole("radio", { name: /^严格模式/ }));
  fireEvent.click(screen.getByRole("button", { name: t.continue }));
  expect((await screen.findByRole("alert")).textContent).toContain("disk unavailable");
  expect((screen.getByRole("radio", { name: /^严格模式/ }) as HTMLInputElement).checked).toBe(true);
  fireEvent.click(screen.getByRole("button", { name: t.continue }));
  await waitFor(() => expect(save).toHaveBeenCalledTimes(2));
});

it.each([403, 409])("explains an existing %s permission boundary", async (status) => {
  const save = vi.fn(async () => { throw new ApprovalModeFailure(status, "denied"); });
  const { t } = mount("zh-CN", save);
  fireEvent.click(screen.getByRole("button", { name: t.continue }));
  expect((await screen.findByRole("alert")).textContent).toContain(status === 403 ? "本机回环地址" : "只读策略");
});

it.each(["zh-CN", "en", "ja"] as const)("localizes the three choices in %s", (language) => {
  const { t } = mount(language);
  expect(screen.getByRole("dialog", { name: t.title })).toBeTruthy();
  expect(screen.getByRole("radio", { name: new RegExp(`^${t.smart}`) })).toBeTruthy();
  expect(screen.getByRole("button", { name: messages[language].cancel })).toBeTruthy();
});
