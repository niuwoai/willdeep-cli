// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import { Markdown } from "./Markdown";
import { messages } from "./i18n";
import { markdownCopy, queueCopyLabels } from "./markdownCopy";

afterEach(cleanup);

it("puts a copy control on a fenced block and confirms the copied text", async () => {
  const writeText = vi.fn(async () => {});
  Object.assign(navigator, { clipboard: { writeText } });
  render(<Markdown copy={markdownCopy(messages["zh-CN"])} content={"说明\n\n```sh\nssh-keygen -t ed25519\n```\n"} />);
  fireEvent.click(screen.getByRole("button", { name: "复制" }));
  expect(writeText).toHaveBeenCalledWith("ssh-keygen -t ed25519");
  expect((await screen.findByRole("status")).textContent).toBe("已复制");
  expect(screen.getByRole("button", { name: "复制" }).querySelector("svg")).toBeTruthy();
});

it("leaves inline code without a copy control and reports a failed copy", async () => {
  Object.assign(navigator, { clipboard: { writeText: vi.fn(async () => { throw new Error("denied"); }) } });
  document.execCommand = () => false;
  render(<Markdown copy={queueCopyLabels("Copy", "Copied", "Couldn't copy")} content={"用 `inline` 就行\n\n```\nline one\n```"} />);
  expect(screen.getAllByRole("button", { name: "Copy" })).toHaveLength(1);
  fireEvent.click(screen.getByRole("button", { name: "Copy" }));
  expect((await screen.findByRole("status")).textContent).toBe("Couldn't copy");
});
