// @vitest-environment jsdom
import { afterEach, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { ChakraProvider, defaultSystem } from "@chakra-ui/react";
import { ConfigurationSettings } from "./ConfigurationSettings";
import { messages } from "./i18n";
import { reasoningText } from "./reasoningMessages";
import type { ReasoningProfile } from "./modelApi";

const t = messages["zh-CN"];
const reasoning = reasoningText("zh-CN");
afterEach(() => { cleanup(); vi.unstubAllGlobals(); });

function mount(profile: ReasoningProfile, compact = false) {
  const snapshot = {
    revision: "first", default_provider: "preview", config_path: "/fixture/config.toml",
    providers: [{ id: "preview", provider: "openai-compatible", api: "chat-completions", api_base: compact ? "https://provider.example/v1" : "", model: "test-model", api_key_configured: true, reasoning: profile }],
  };
  const updates: Record<string, unknown>[] = [];
  vi.stubGlobal("fetch", vi.fn(async (_url: string, init?: RequestInit) => {
    if (init?.method === "PUT") {
      updates.push(JSON.parse(String(init.body)) as Record<string, unknown>);
      return Response.json({ ...snapshot, revision: "second" });
    }
    return Response.json(snapshot);
  }));
  render(<ChakraProvider value={defaultSystem}>
    <ConfigurationSettings messages={t} language="zh-CN" reasoningOnly={!compact} compact={compact} />
  </ChakraProvider>);
  return updates;
}

it.each(["balanced", "__proto__"])("preserves all existing scalar mappings with default %s in a reasoning-only save", async (defaultLevel) => {
  const profile: ReasoningProfile = {
    default_level: defaultLevel, parameter: "reasoning_effort",
    levels: { balanced: "medium", ["__proto__"]: "high", budget: 4096, disabled: false },
  };
  const updates = mount(profile);
  await waitFor(() => expect(screen.getByRole<HTMLInputElement>("textbox", { name: reasoning.defaultLevel }).value).toBe(defaultLevel));
  expect(screen.queryByRole("textbox", { name: t.configApiBase })).toBeNull();
  fireEvent.click(screen.getByRole("button", { name: t.configSave }));
  await screen.findByRole("status");
  expect(updates).toHaveLength(1);
  expect(updates[0].reasoning_only).toBe(true);
  expect(updates[0].reasoning).toEqual(profile);
  expect(Object.hasOwn((updates[0].reasoning as ReasoningProfile).levels, "__proto__")).toBe(true);
});

it("omits reasoning from quick setup to retain existing mappings", async () => {
  const updates = mount({ default_level: "deep", parameter: null, levels: { deep: "high" } }, true);
  await waitFor(() => expect(screen.getByRole<HTMLInputElement>("textbox", { name: t.configModel }).value).toBe("test-model"));
  fireEvent.click(screen.getByRole("button", { name: t.setupDone }));
  await screen.findByRole("status");
  expect(updates).toHaveLength(1);
  expect(Object.hasOwn(updates[0], "reasoning")).toBe(false);
  expect(Object.hasOwn(updates[0], "reasoning_only")).toBe(false);
});

it("rejects a duplicate logical level without sending a partial mapping", async () => {
  const updates = mount({ default_level: "balanced", parameter: null, levels: { balanced: "medium", deep: "high" } });
  await waitFor(() => expect(screen.getAllByRole("textbox", { name: reasoning.levelName })).toHaveLength(2));
  fireEvent.change(screen.getAllByRole("textbox", { name: reasoning.levelName })[1], { target: { value: "balanced" } });
  fireEvent.click(screen.getByRole("button", { name: t.configSave }));
  expect((await screen.findByRole("alert")).textContent).toContain(reasoning.duplicateLevel);
  expect(updates).toEqual([]);
});
