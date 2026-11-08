// @vitest-environment jsdom
import { afterEach, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { ReasoningPicker } from "./ReasoningPicker";
import { reasoningText } from "./reasoningMessages";
import type { ReasoningProfile } from "./modelApi";

const t = reasoningText("zh-CN");
const clients: QueryClient[] = [];
afterEach(() => {
  cleanup();
  clients.forEach((client) => client.clear());
  clients.length = 0;
  vi.unstubAllGlobals();
});

function mount(profile: ReasoningProfile | null, refused = false) {
  let level: string | null = "deep";
  const updates: unknown[] = [];
  const onChanged = vi.fn();
  vi.stubGlobal("fetch", vi.fn(async (url: string, init?: RequestInit) => {
    if (init?.method === "POST") {
      const body = JSON.parse(String(init.body)) as { reasoning_level: string | null };
      updates.push(body);
      if (refused) return Response.json({ error: "request refused" }, { status: 400 });
      level = body.reasoning_level;
      return new Response(null, { status: 204 });
    }
    if (url === "/api/settings/providers") return Response.json({ providers: [{ id: "next", reasoning: profile }] });
    if (url === "/api/models/default") return Response.json({ model: null, writable: true });
    return Response.json({ provider_id: "next", reasoning_level: level });
  }));
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  clients.push(client);
  const view = render(<QueryClientProvider client={client}>
    <ReasoningPicker sessionId="session" current="deep" currentProviderId="next" language="zh-CN" onChanged={onChanged} />
  </QueryClientProvider>);
  const details = view.container.querySelector("details")!;
  details.open = true;
  fireEvent(details, new Event("toggle"));
  return { ...view, onChanged, updates };
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

it("keeps the override and reports failure when clearing is rejected", async () => {
  const { onChanged, updates } = mount(null, true);
  const select = await screen.findByRole<HTMLSelectElement>("combobox", { name: t.title });
  fireEvent.change(select, { target: { value: "" } });
  expect((await screen.findByRole("alert")).textContent).toContain("request refused");
  expect(onChanged).not.toHaveBeenCalled();
  expect(updates).toEqual([{ reasoning_level: null }]);
  expect(select.value).toBe("deep");
});
