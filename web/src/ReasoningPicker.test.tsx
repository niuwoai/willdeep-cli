// @vitest-environment jsdom
import { afterEach, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { ReasoningPicker } from "./ReasoningPicker";
import { reasoningText } from "./reasoningMessages";
import type { ReasoningProfile, ReasoningProvider } from "./modelApi";

const t = reasoningText("zh-CN");
const clients: QueryClient[] = [];
afterEach(() => {
  cleanup();
  clients.forEach((client) => client.clear());
  clients.length = 0;
  vi.unstubAllGlobals();
});

function mount(profile: ReasoningProfile | null, refused = false, extra: Partial<ReasoningProvider> = {}, initial: string | null = "deep", gate?: Promise<void>) {
  let level: string | null = initial;
  const updates: unknown[] = [];
  const onChanged = vi.fn();
  vi.stubGlobal("fetch", vi.fn(async (url: string, init?: RequestInit) => {
    if (init?.method === "POST") {
      const body = JSON.parse(String(init.body)) as { reasoning_level: string | null };
      updates.push(body);
      level = body.reasoning_level;
      if (gate) await gate;
      if (refused) return Response.json({ error: "request refused" }, { status: 400 });
      return new Response(null, { status: 204 });
    }
    if (url === "/api/settings/providers") return Response.json({ providers: [{ id: "next", reasoning: profile, ...extra }] });
    if (url === "/api/models/default") return Response.json({ model: null, writable: true });
    return Response.json({ provider_id: "next", reasoning_level: level });
  }));
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  clients.push(client);
  const view = render(<QueryClientProvider client={client}>
    <ReasoningPicker sessionId="session" current={initial} currentProviderId="next" language="zh-CN" onChanged={onChanged} />
  </QueryClientProvider>);
  const details = view.container.querySelector("details")!;
  details.open = true;
  fireEvent(details, new Event("toggle"));
  return { ...view, onChanged, updates, details, setProfile: (next: ReasoningProfile) => { profile = next; } };
}

it.each([null, { default_level: null, parameter: null, levels: {} }])("can clear a previous override after switching to an unmapped provider or deleting its mapping (%j)", async (profile) => {
  const { onChanged, updates } = mount(profile);
  const select = await screen.findByRole<HTMLSelectElement>("combobox", { name: t.title });
  expect(select.value).toBe("deep");
  expect(screen.getByText(t.notConfigured)).toBeTruthy();
  fireEvent.change(select, { target: { value: "" } });
  await waitFor(() => expect(onChanged).toHaveBeenCalledExactlyOnceWith(null));
  expect(updates).toEqual([{ reasoning_level: null }]);
  await waitFor(() => expect(screen.queryByRole("combobox", { name: t.title })).toBeNull());
});

it("refreshes provider levels when reopening after configuration changes", async () => {
  const { details, setProfile } = mount({ default_level: "deep", parameter: null, levels: { deep: "high" } });
  await screen.findByRole("option", { name: "deep" });
  details.open = false;
  fireEvent(details, new Event("toggle"));
  setProfile({ default_level: "quick", parameter: null, levels: { quick: "low", deep: "high" } });
  details.open = true;
  fireEvent(details, new Event("toggle"));
  expect(await screen.findByRole("option", { name: "quick" })).toBeTruthy();
});

it("keeps the override and reports failure when clearing is rejected", async () => {
  const { onChanged, updates } = mount(null, true);
  const select = await screen.findByRole<HTMLSelectElement>("combobox", { name: t.title });
  fireEvent.change(select, { target: { value: "" } });
  expect((await screen.findByRole("alert")).textContent).toContain("request refused");
  expect(onChanged).not.toHaveBeenCalled();
  expect(updates).toEqual([{ reasoning_level: null }]);
  expect(select.value).toBe("deep");
});

it("renders a slider for the built-in ladder when the provider has no mapping", async () => {
  const { onChanged, updates } = mount(null, false, { reasoning_levels: ["low", "medium", "high", "xhigh"], reasoning_builtin: true }, null);
  const slider = await screen.findByRole<HTMLInputElement>("slider", { name: t.title });
  expect(slider.max).toBe("4");
  expect(slider.value).toBe("0");
  expect(screen.getByText(t.builtinNotice)).toBeTruthy();
  expect(screen.getByText(t.levelXhigh)).toBeTruthy();
  // 拖到第 2 档（medium）松手才提交。
  fireEvent.change(slider, { target: { value: "2" } });
  expect(updates).toEqual([]);
  fireEvent.pointerUp(slider);
  await waitFor(() => expect(onChanged).toHaveBeenCalledExactlyOnceWith("medium"));
  expect(updates).toEqual([{ reasoning_level: "medium" }]);
});

it("rolls the slider back and reports the provider error when the update fails", async () => {
  const { onChanged, updates } = mount(null, true, { reasoning_levels: ["low", "medium", "high", "xhigh"], reasoning_builtin: true }, null);
  const slider = await screen.findByRole<HTMLInputElement>("slider", { name: t.title });
  fireEvent.change(slider, { target: { value: "2" } });
  fireEvent.pointerUp(slider);
  expect((await screen.findByRole("alert")).textContent).toContain("request refused");
  expect(onChanged).not.toHaveBeenCalled();
  expect(updates).toEqual([{ reasoning_level: "medium" }]);
  // 失败回滚到已确认的「跟随默认」，不把 thumb 留在未保存的档位。
  expect(slider.value).toBe("0");
});

it("orders numeric mappings into a slider and keeps unordered names on the select", async () => {
  const numeric = mount({ default_level: "low", parameter: null, levels: { high: 8000, low: 1000 } }, false, { reasoning_levels: ["high", "low"], reasoning_builtin: false }, "high");
  const slider = await screen.findByRole<HTMLInputElement>("slider", { name: t.title });
  expect(slider.max).toBe("2");
  // 数字升序：low 在前，当前值 high 落在第 2 档。
  expect(screen.getByText(t.levelLow)).toBeTruthy();
  expect(slider.value).toBe("2");
  numeric.unmount();

  mount({ default_level: "balanced", parameter: null, levels: { economy: "low", balanced: "medium", deep: "high" } }, false, { reasoning_levels: ["balanced", "deep", "economy"], reasoning_builtin: false }, "balanced");
  expect(await screen.findByRole<HTMLSelectElement>("combobox", { name: t.title })).toBeTruthy();
  expect(screen.queryByRole("slider", { name: t.title })).toBeNull();
});

it("debounces arrow-key changes so one release worth of keys saves once", async () => {
  const { onChanged, updates } = mount(null, false, { reasoning_levels: ["low", "medium", "high", "xhigh"], reasoning_builtin: true }, null);
  const slider = await screen.findByRole<HTMLInputElement>("slider", { name: t.title });
  fireEvent.change(slider, { target: { value: "1" } });
  fireEvent.keyUp(slider);
  fireEvent.change(slider, { target: { value: "3" } });
  fireEvent.keyUp(slider);
  // 350ms 内没有新按键，才把最后停留的 high 提交一次。
  expect(updates).toEqual([]);
  await waitFor(() => expect(onChanged).toHaveBeenCalledExactlyOnceWith("high"), { timeout: 1500 });
  expect(updates).toEqual([{ reasoning_level: "high" }]);
});

it("queues the latest slider target while a save is still in flight", async () => {
  let release!: () => void;
  const gate = new Promise<void>((resolve) => { release = resolve; });
  const { onChanged, updates } = mount(null, false, { reasoning_levels: ["low", "medium", "high", "xhigh"], reasoning_builtin: true }, null, gate);
  const slider = await screen.findByRole<HTMLInputElement>("slider", { name: t.title });
  fireEvent.change(slider, { target: { value: "1" } });
  fireEvent.pointerUp(slider);
  await waitFor(() => expect(updates).toEqual([{ reasoning_level: "low" }]));
  // 保存中控件仍可用，第二次选择不会被 disabled 吞掉。
  expect(slider.disabled).toBe(false);
  fireEvent.change(slider, { target: { value: "3" } });
  fireEvent.pointerUp(slider);
  expect(updates).toHaveLength(1);
  release();
  await waitFor(() => expect(updates).toHaveLength(2));
  expect(updates).toEqual([{ reasoning_level: "low" }, { reasoning_level: "high" }]);
  await waitFor(() => expect(onChanged).toHaveBeenLastCalledWith("high"));
});

it("marks a session level the current provider cannot accept as unavailable", async () => {
  const { onChanged, updates } = mount(null, false, { reasoning_levels: ["low", "medium", "high", "xhigh"], reasoning_builtin: true }, "deep");
  const slider = await screen.findByRole<HTMLInputElement>("slider", { name: t.title });
  expect(screen.getByText(t.staleOverride.replace("{level}", "deep"))).toBeTruthy();
  expect(screen.getByText(`${t.current}: deep`)).toBeTruthy();
  expect(screen.getByText(`deep · ${t.unavailableTag}`)).toBeTruthy();
  // 默认档 + 四个内置档 + 旧覆盖 = 第 5 档；拉到默认档即可清除。
  expect(slider.value).toBe("5");
  fireEvent.change(slider, { target: { value: "0" } });
  fireEvent.pointerUp(slider);
  await waitFor(() => expect(onChanged).toHaveBeenCalledExactlyOnceWith(null));
  expect(updates).toEqual([{ reasoning_level: null }]);
});
