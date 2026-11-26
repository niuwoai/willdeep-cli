// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { afterEach, expect, it, vi } from "vitest";
import { useState } from "react";
import { DesktopChangesPanel, DesktopChangesToggle } from "./DesktopChanges";
import { useDesktopChanges, type ChangedFile } from "./useDesktopChanges";

const workspace = "/fixture/workspace";
const session = "fixture-session";
const tracked: ChangedFile = { path: "run.txt", old_path: null, kind: "modified", staged: true, unstaged: true, additions: 2, deletions: 2, binary: false };
const other: ChangedFile = { path: "prior.txt", old_path: null, kind: "untracked", staged: false, unstaged: true, additions: 1, deletions: 0, binary: true };

function Workbench() {
  const [open, setOpen] = useState(false);
  const query = useDesktopChanges(workspace, session, 0, open);
  return <>
    <DesktopChangesToggle language="zh-CN" open={open} count={query.data?.snapshot?.files.length ?? 0} onToggle={() => setOpen(!open)} />
    {open && <DesktopChangesPanel language="zh-CN" workspace={workspace} session={session} query={query} onClose={() => setOpen(false)} />}
  </>;
}

function openWorkbench() {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(<QueryClientProvider client={client}><Workbench /></QueryClientProvider>);
  const trigger = screen.getByRole("button", { name: "工作区改动" });
  expect(trigger.getAttribute("aria-expanded")).toBe("false");
  fireEvent.click(trigger);
}

function changes(id: string, sessionPaths: string[] | null = [tracked.path]) {
  return Response.json({ git: true, branch: "main", session_paths: sessionPaths, snapshot: { id, files: [tracked, other] } });
}

function params(request: string) { return new URL(request, "http://localhost").searchParams; }

afterEach(() => { cleanup(); vi.unstubAllGlobals(); });

it("separates files with session evidence and switches snapshot-bound staged and unstaged patches", async () => {
  const responses = vi.fn(async (request: string) => {
    if (request.startsWith("/api/workspaces/changes/file?")) {
      const query = params(request);
      expect([query.get("workspace"), query.get("session"), query.get("snapshot_id"), query.get("path")]).toEqual([workspace, session, "snapshot-1", tracked.path]);
      return Response.json({ content: `@@ -1 +1 @@\n-old\n+${query.get("area")}\n` });
    }
    expect(request.startsWith("/api/workspaces/changes?")).toBe(true);
    return changes("snapshot-1");
  });
  vi.stubGlobal("fetch", responses);
  openWorkbench();

  const sessionGroup = await screen.findByRole("region", { name: "本会话涉及" });
  const otherGroup = screen.getByRole("region", { name: "其他或归属未知的改动" });
  expect(within(sessionGroup).queryByText(other.path)).toBeNull();
  expect(within(otherGroup).queryByText(tracked.path)).toBeNull();
  fireEvent.click(within(sessionGroup).getByRole("button", { name: /^run.txt/ }));
  expect(await screen.findByText("+staged")).toBeTruthy();
  expect(screen.getByText("记录只标记涉及的文件。下方差异可能包含其他来源的改动。")).toBeTruthy();
  fireEvent.click(screen.getByRole("button", { name: "未暂存" }));
  expect(await screen.findByText("+unstaged")).toBeTruthy();
  fireEvent.click(within(otherGroup).getByRole("button", { name: /^prior.txt/ }));
  expect(screen.getByText("二进制文件")).toBeTruthy();
  expect(responses.mock.calls.filter(([path]) => path.includes("/file?"))).toHaveLength(2);
  fireEvent.keyDown(screen.getByRole("complementary"), { key: "Escape" });
  expect(screen.queryByRole("complementary")).toBeNull();
});

it("requires a fresh snapshot after a conflict and keeps missing attribution outside the session group", async () => {
  let id = "stale";
  vi.stubGlobal("fetch", vi.fn(async (request: string) => {
    if (request.includes("/file?")) {
      return params(request).get("snapshot_id") === "stale"
        ? new Response(null, { status: 409 }) : Response.json({ content: "+fresh\n" });
    }
    return changes(id, null);
  }));
  openWorkbench();
  const otherGroup = await screen.findByRole("region", { name: "其他或归属未知的改动" });
  expect(screen.queryByRole("region", { name: "本会话涉及" })).toBeNull();
  fireEvent.click(within(otherGroup).getByRole("button", { name: /^run.txt/ }));
  expect((await screen.findByRole("alert")).textContent).toBe("工作区已变化，请刷新改动。");
  id = "fresh";
  fireEvent.click(screen.getByRole("button", { name: "刷新改动" }));
  expect(await screen.findByText("+fresh")).toBeTruthy();
});
