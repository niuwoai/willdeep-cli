// @vitest-environment jsdom
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { Markdown } from "./Markdown";
import { messages } from "./i18n";
import { markdownCopy, queueCopyLabels } from "./markdownCopy";

const dialogPrototype = HTMLDialogElement.prototype;
const dialogMethods = {
  showModal: Object.getOwnPropertyDescriptor(dialogPrototype, "showModal"),
  close: Object.getOwnPropertyDescriptor(dialogPrototype, "close"),
};
beforeEach(() => {
  Object.defineProperties(dialogPrototype, {
    showModal: { configurable: true, value(this: HTMLDialogElement) { this.open = true; } },
    close: { configurable: true, value(this: HTMLDialogElement) { this.open = false; } },
  });
});
afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
  for (const [name, method] of Object.entries(dialogMethods)) {
    if (method) Object.defineProperty(dialogPrototype, name, method);
    else Reflect.deleteProperty(dialogPrototype, name);
  }
});

it("puts a copy control on a fenced block and confirms the copied text", async () => {
  const writeText = vi.fn(async () => {});
  Object.assign(navigator, { clipboard: { writeText } });
  render(<Markdown copy={markdownCopy(messages["zh-CN"])} content={"说明\n\n```sh\nssh-keygen -t ed25519\n```\n"} />);
  fireEvent.click(screen.getByRole("button", { name: "复制" }));
  expect(writeText).toHaveBeenCalledWith("ssh-keygen -t ed25519");
  expect((await screen.findByRole("status")).textContent).toBe("已复制");
});

it.each([
  ["input", "  command  with spaces"],
  ["textarea", "## Summary\n\n- first line\n  - indented line\n"],
])("copies a %s card with its exact whitespace", async (mode, text) => {
  const writeText = vi.fn(async () => {});
  Object.assign(navigator, { clipboard: { writeText } });
  render(<Markdown copy={markdownCopy(messages["zh-CN"])} content={`\`\`\`${mode}\n${text}\n\`\`\``} />);
  const button = screen.getByRole("button", { name: "复制" });
  const card = button.closest(`.markdown-copy-field-${mode}`);
  expect(card?.querySelector(".markdown-copy-field-body")?.textContent).toBe(text);
  fireEvent.click(button);
  expect(writeText).toHaveBeenCalledOnce();
  expect(writeText).toHaveBeenCalledWith(text);
  expect((await screen.findByRole("status")).textContent).toBe("已复制");
});

it("adds a bare open icon to HTTP(S) autolinks and titled Markdown links", () => {
  render(<Markdown copy={markdownCopy(messages.ja)} content={"https://example.org [レポート](http://localhost:9879/report.md) [文書内](#header) [危険](javascript:alert(1))"} />);
  for (const name of ["https://example.org", "レポート"]) {
    const link = screen.getByRole("link", { name });
    expect(link.querySelector(".markdown-link-open")).toBeTruthy();
    expect(link.getAttribute("title")).toBe("ブラウザーで開く");
    expect(link.getAttribute("target")).toBe("_blank");
    expect(link.getAttribute("rel")).toBe("noopener noreferrer");
  }
  expect(screen.getByRole("link", { name: "文書内" }).querySelector("svg")).toBeNull();
  expect(screen.queryByRole("link", { name: "危険" })).toBeNull();
});

function renderSession(content: string) {
  const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(<QueryClientProvider client={queryClient}>
    <Markdown copy={markdownCopy(messages.en)} content={content} sessionId="current-session" />
  </QueryClientProvider>);
}

it("lazily previews a Markdown file through the authorized session API", async () => {
  const fetchFile = vi.fn(async () => Response.json({ content: "# The report\n\n[External](https://example.org)\n\n![Image](https://example.org/image.png)" }));
  vi.stubGlobal("fetch", fetchFile);
  renderSession("See [report](docs/report%20one.md#L2) for details.");
  expect(fetchFile).not.toHaveBeenCalled();
  const chip = screen.getByRole("button", { name: /Preview file.*report one\.md/ });
  expect(chip.getAttribute("title")).toBe("docs/report one.md");
  fireEvent.click(chip);
  expect(fetchFile).toHaveBeenCalledWith("/api/sessions/current-session/file-preview?path=docs%2Freport+one.md", { signal: expect.any(AbortSignal) });
  const dialog = screen.getByRole("dialog", { name: "report one.md" });
  expect(await within(dialog).findByRole("heading", { name: "The report" })).toBeTruthy();
  expect(within(dialog).queryByRole("link")).toBeNull();
  expect(within(dialog).queryByRole("img")).toBeNull();
  fireEvent.click(within(dialog).getByRole("button", { name: "Close preview" }));
  expect(screen.queryByRole("dialog")).toBeNull();
  expect(document.activeElement).toBe(chip);
});

it.each(["page.html", "render.svg", "main.js"])("displays %s as inert source", async (name) => {
  const source = "<script>console.info('from preview')</script>";
  vi.stubGlobal("fetch", vi.fn(async () => Response.json({ content: source })));
  renderSession(`[source](${name})`);
  fireEvent.click(screen.getByRole("button", { name: `Preview file · ${name}` }));
  const dialog = screen.getByRole("dialog", { name });
  expect((await within(dialog).findByText(source)).tagName).toBe("PRE");
  expect(dialog.querySelector("script, iframe, object, embed")).toBeNull();
});

it("shows a local error for an unavailable file and leaves unsafe paths inert", async () => {
  const fetchFile = vi.fn(async () => new Response(null, { status: 404 }));
  vi.stubGlobal("fetch", fetchFile);
  renderSession("[Missing](missing.md) [Traversal](../hidden.md) [Remote](file://host/report.md)");
  expect(screen.getAllByRole("button")).toHaveLength(1);
  expect(screen.queryByRole("link")).toBeNull();
  fireEvent.click(screen.getByRole("button", { name: /missing.md/ }));
  expect((await screen.findByRole("alert")).textContent).toContain("Could not preview this file");
});

it("leaves workspace paths inert when there is no session", () => {
  render(<Markdown copy={markdownCopy(messages.en)} content="[App.tsx](src/App.tsx)" />);
  expect(screen.getByText("App.tsx").tagName).toBe("SPAN");
});

it("leaves inline code without a copy control and reports a failed copy", async () => {
  Object.assign(navigator, { clipboard: { writeText: vi.fn(async () => { throw new Error("denied"); }) } });
  document.execCommand = () => false;
  render(<Markdown copy={queueCopyLabels("Copy", "Copied", "Couldn't copy")} content={"用 `inline` 就行\n\n```\nline one\n```"} />);
  expect(screen.getAllByRole("button", { name: "Copy" })).toHaveLength(1);
  fireEvent.click(screen.getByRole("button", { name: "Copy" }));
  expect((await screen.findByRole("status")).textContent).toBe("Couldn't copy");
});
