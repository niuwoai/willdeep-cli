// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { afterEach, expect, it, vi } from "vitest";
import { useState } from "react";
import { DesktopChangesPanel, DesktopChangesToggle, useDesktopChanges } from "./DesktopChanges";

const workspace = "/fixture/workspace";
const session = "fixture-session";

function Workbench() {
  const [open, setOpen] = useState(false);
  const query = useDesktopChanges(workspace, session, 0, true);
  return <>
    <DesktopChangesToggle language="zh-CN" open={open} count={query.data?.snapshot?.files.length ?? 0} onToggle={() => setOpen(!open)} />
    {open && <DesktopChangesPanel language="zh-CN" workspace={workspace} session={session} query={query} onClose={() => setOpen(false)} />}
  </>;
}

afterEach(() => { cleanup(); vi.unstubAllGlobals(); });

it("opens the desktop file diff with the visible snapshot and keeps workspace changes distinct", async () => {
  const responses = vi.fn(async (request: string) => {
    if (request.startsWith("/api/desktop/changes/file?")) {
      expect(new URL(request, "http://localhost").searchParams.get("snapshot_id")).toBe("snapshot-1");
      expect(new URL(request, "http://localhost").searchParams.get("session")).toBe(session);
      return Response.json({ content: "@@ -1 +1 @@\n-old\n+new\n" });
    }
    return Response.json({
      git: true, branch: "main", session_paths: ["run.txt"],
      snapshot: { id: "snapshot-1", files: [
        { path: "run.txt", old_path: null, additions: 1, deletions: 1, binary: false },
        { path: "prior.txt", old_path: null, additions: 2, deletions: 0, binary: true },
      ] },
    });
  });
  vi.stubGlobal("fetch", responses);
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(<QueryClientProvider client={client}><Workbench /></QueryClientProvider>);
  const trigger = await screen.findByRole("button", { name: "改动 2 个文件" }, { timeout: 5000 });
  expect(trigger.getAttribute("aria-expanded")).toBe("false");
  fireEvent.click(trigger);
  expect(screen.getByRole("complementary", { name: "工作区改动" })).toBeTruthy();
  expect(screen.getByText("本会话涉及")).toBeTruthy();
  fireEvent.click(screen.getByRole("button", { name: /^run.txt/ }));
  expect(await screen.findByText("+new", {}, { timeout: 5000 })).toBeTruthy();
  fireEvent.click(screen.getByRole("button", { name: /^prior.txt/ }));
  expect(screen.getByText("二进制文件")).toBeTruthy();
  expect(responses.mock.calls.filter(([path]) => path.includes("/file?"))).toHaveLength(1);
  fireEvent.keyDown(screen.getByRole("complementary"), { key: "Escape" });
  expect(screen.queryByRole("complementary")).toBeNull();
}, 15_000);
