// @vitest-environment jsdom
import type { PropsWithChildren } from "react";
import { act, cleanup, renderHook, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { afterEach, describe, expect, it, vi } from "vitest";
import { needsWorkspaceApproval, useWorkspaceApproval, type WorkspaceEntry } from "./useWorkspaceApproval";
import { workspaceListKey, type WorkspaceSummary } from "./workspaceApprovalApi";

const first: WorkspaceSummary = { id: "a", path: "/first", name: "first", active: false, access: "smart", approval_mode_configured: false };
const other: WorkspaceSummary = { ...first, id: "b", path: "/other", name: "other" };
afterEach(() => { cleanup(); vi.unstubAllGlobals(); });

function mount() {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false, staleTime: Infinity }, mutations: { retry: false } } });
  client.setQueryData(workspaceListKey, [first, other]);
  const entered = vi.fn<(workspace: WorkspaceSummary, entry: WorkspaceEntry) => void>();
  const wrapper = ({ children }: PropsWithChildren) => <QueryClientProvider client={client}>{children}</QueryClientProvider>;
  return { ...renderHook(() => useWorkspaceApproval(entered), { wrapper }), entered, client };
}

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((done) => { resolve = done; });
  return { promise, resolve };
}

describe("workspace approval entry", () => {
  it("cancels without entering a directory or saving a selection", () => {
    const fetcher = vi.fn(); vi.stubGlobal("fetch", fetcher);
    const { result, entered } = mount();
    act(() => result.current.requestEntry(first, { fresh: true }));
    expect(result.current.pending?.workspace.path).toBe("/first");
    expect(entered).not.toHaveBeenCalled();
    act(() => result.current.cancel());
    expect(result.current.pending).toBeNull();
    expect(fetcher).not.toHaveBeenCalled();
    expect(entered).not.toHaveBeenCalled();
  });

  it.each(["strict", "smart", "workspace_write", "full_access", "read_only"] as const)("preserves the configured %s choice", (access) => {
    const { result, entered } = mount();
    const workspace = { ...first, access, approval_mode_configured: true };
    act(() => result.current.requestEntry(workspace));
    expect(result.current.pending).toBeNull();
    expect(entered).toHaveBeenCalledWith(workspace, {});
  });

  it("never raises a read-only workspace and treats a legacy missing marker as configured", () => {
    expect(needsWorkspaceApproval({ ...first, access: "read_only" })).toBe(false);
    const legacy = { ...first, approval_mode_configured: undefined };
    expect(needsWorkspaceApproval(legacy as unknown as WorkspaceSummary)).toBe(false);
  });

  it("saves the selected value and preserves the requested session restoration", async () => {
    const fetcher = vi.fn(async () => Response.json({ ...first, access: "strict", approval_mode_configured: true }));
    vi.stubGlobal("fetch", fetcher);
    const { result, entered, client } = mount();
    const entry = { session: { id: "existing-session", turnId: "existing-turn" } };
    act(() => result.current.requestEntry(first, entry));
    await act(async () => { await result.current.apply("strict", false); });
    expect(fetcher).toHaveBeenCalledWith("/api/workspaces/approval-mode", expect.objectContaining({ body: JSON.stringify({ path: "/first", approval_mode: "strict", confirm: false }) }));
    expect(entered).toHaveBeenCalledWith(expect.objectContaining({ access: "strict", approval_mode_configured: true }), entry);
    expect(result.current.pending).toBeNull();
    expect(client.getQueryData<WorkspaceSummary[]>(workspaceListKey)?.find((item) => item.path === "/first")?.approval_mode_configured).toBe(true);
  });

  it("enters the requested directory after a deferred approval save completes", async () => {
    const response = deferred<Response>();
    const fetcher = vi.fn(() => response.promise);
    vi.stubGlobal("fetch", fetcher);
    const { result, entered } = mount();
    act(() => result.current.requestEntry(first));
    let saving!: Promise<void>;
    act(() => { saving = result.current.apply("strict", false); });
    await waitFor(() => expect(fetcher).toHaveBeenCalledOnce());
    expect(entered).not.toHaveBeenCalled();

    await act(async () => {
      response.resolve(Response.json({ ...first, access: "strict", approval_mode_configured: true }));
      await saving;
    });
    expect(entered).toHaveBeenCalledWith(expect.objectContaining({ access: "strict", approval_mode_configured: true }), {});
    expect(result.current.pending).toBeNull();
  });

  it("cannot submit full access without explicit confirmation", async () => {
    const fetcher = vi.fn(); vi.stubGlobal("fetch", fetcher);
    const { result, entered } = mount();
    act(() => result.current.requestEntry(first));
    await act(async () => { await result.current.apply("full_access", false); });
    expect(fetcher).not.toHaveBeenCalled();
    expect(entered).not.toHaveBeenCalled();
    expect(result.current.pending).not.toBeNull();
  });

  it("keeps the guide available after a save failure and permits retry", async () => {
    const fetcher = vi.fn().mockResolvedValueOnce(Response.json({ error: "disk unavailable" }, { status: 500 })).mockResolvedValueOnce(Response.json({ ...first, approval_mode_configured: true }));
    vi.stubGlobal("fetch", fetcher);
    const { result, entered } = mount();
    act(() => result.current.requestEntry(first));
    await act(async () => { await expect(result.current.apply("smart", false)).rejects.toThrow("disk unavailable"); });
    expect(result.current.pending?.workspace.path).toBe("/first");
    expect(entered).not.toHaveBeenCalled();
    await act(async () => { await result.current.apply("smart", false); });
    expect(entered).toHaveBeenCalledOnce();
  });

  it.each(["cancel", "other-directory"])("ignores a late save response after %s", async (action) => {
    const response = deferred<Response>();
    vi.stubGlobal("fetch", vi.fn(() => response.promise));
    const { result, entered } = mount();
    act(() => result.current.requestEntry(first));
    let saving!: Promise<void>;
    act(() => { saving = result.current.apply("full_access", true); });
    act(() => { if (action === "cancel") result.current.cancel(); else result.current.requestEntry(other); });
    await act(async () => { response.resolve(Response.json({ ...first, access: "full_access", approval_mode_configured: true })); await saving; });
    expect(entered).not.toHaveBeenCalled();
    expect(result.current.pending?.workspace.path ?? null).toBe(action === "cancel" ? null : "/other");
  });

  it("rejects a response naming another directory", async () => {
    vi.stubGlobal("fetch", vi.fn(async () => Response.json({ ...other, access: "full_access", approval_mode_configured: true })));
    const { result, entered } = mount();
    act(() => result.current.requestEntry(first));
    await act(async () => { await expect(result.current.apply("smart", false)).rejects.toThrow(); });
    expect(entered).not.toHaveBeenCalled();
    expect(result.current.pending?.workspace.path).toBe("/first");
  });
});
