// @vitest-environment jsdom
import { act, cleanup, renderHook } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import { useComposerReferences, type Reference } from "./useComposerReferences";

afterEach(() => { cleanup(); vi.useRealTimers(); vi.unstubAllGlobals(); });
const response = (references: Reference[]) => ({ ok: true, json: async () => ({ references }) });

it("loads candidates for a bare at sign without changing the draft", async () => {
  vi.useFakeTimers();
  const entries: Reference[] = [{ kind: "agent", name: "Reviewer", value: "@agent:reviewer" }];
  const fetch = vi.fn().mockResolvedValue(response(entries));
  vi.stubGlobal("fetch", fetch);
  const onError = vi.fn();
  const { result } = renderHook(() => useComposerReferences("/workspace", "@", "Failed", onError));
  act(() => result.current.update("@", 1));
  await act(async () => { await vi.advanceTimersByTimeAsync(150); });
  expect(fetch).toHaveBeenCalledWith("/api/composer?workspace=%2Fworkspace&reference_query=");
  expect(result.current.query?.query).toBe("");
  expect(result.current.entries).toEqual(entries);
  expect(result.current.loading).toBe(false);
  expect(onError).not.toHaveBeenCalled();
});

it("discards an old workspace response after loading the new workspace", async () => {
  vi.useFakeTimers();
  let finishOld!: (value: ReturnType<typeof response>) => void;
  const old = new Promise<ReturnType<typeof response>>((resolve) => { finishOld = resolve; });
  const current: Reference[] = [{ kind: "file", name: "new.rs", value: '@file:"new.rs"' }];
  const fetch = vi.fn().mockReturnValueOnce(old).mockResolvedValueOnce(response(current));
  vi.stubGlobal("fetch", fetch);
  const { result, rerender } = renderHook(({ workspace }) => useComposerReferences(workspace, "@", "Failed", vi.fn()), { initialProps: { workspace: "/old" } });
  act(() => result.current.update("@", 1));
  await act(async () => { await vi.advanceTimersByTimeAsync(150); });
  rerender({ workspace: "/new" });
  act(() => result.current.update("@", 1));
  await act(async () => { await vi.advanceTimersByTimeAsync(150); });
  await act(async () => { finishOld(response([{ kind: "file", name: "old.rs", value: '@file:"old.rs"' }])); });
  expect(result.current.entries).toEqual(current);
  act(() => result.current.dismiss());
  expect(result.current.query).toBeUndefined();
  expect(result.current.entries).toEqual([]);
});
