// @vitest-environment jsdom
import { afterEach, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { ChakraProvider, defaultSystem } from "@chakra-ui/react";
import userEvent from "@testing-library/user-event";
import { ConfigurationSettings } from "./ConfigurationSettings";
import { messages, type Language } from "./i18n";
import { reasoningText } from "./reasoningMessages";
import type { ReasoningProfile } from "./modelApi";

const t = messages["zh-CN"];
const reasoning = reasoningText("zh-CN");
afterEach(() => { cleanup(); vi.unstubAllGlobals(); });

function mount(profile: ReasoningProfile, compact = false, language: Language = "zh-CN") {
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
    <ConfigurationSettings messages={messages[language]} language={language} reasoningOnly={!compact} compact={compact} />
  </ChakraProvider>);
  return updates;
}

it.each(["zh-CN", "en", "ja"] as const)("fills only the matching logical level field in %s", async (language) => {
  const updates = mount({ default_level: "low", parameter: null, levels: { low: "low" } }, false, language);
  const copy = reasoningText(language);
  await screen.findByRole("button", { name: `preview · ${messages[language].configSetDefault}` });
  const defaultInput = screen.getByRole<HTMLInputElement>("textbox", {
    name: copy.defaultLevel,
    description: copy.defaultLevelHint,
  });
  fireEvent.click(screen.getByRole("button", { name: copy.addLevel }));
  const [existingInput, newInput] = screen.getAllByRole<HTMLInputElement>("textbox", {
    name: copy.levelName,
    description: copy.levelNameHint,
  });
  const defaults = within(screen.getByRole("group", { name: copy.defaultLevel }));
  const newLevels = within(screen.getAllByRole("group", { name: copy.levelName })[1]);
  expect(newLevels.getAllByRole("button").map(button => button.textContent)).toEqual([
    "minimal", "low", "medium", "high", "xhigh", "max",
  ]);
  fireEvent.click(newLevels.getByRole("button", { name: "high" }));
  expect([defaultInput.value, existingInput.value, newInput.value]).toEqual(["low", "low", "high"]);
  fireEvent.click(defaults.getByRole("button", { name: "high" }));
  expect(defaultInput.value).toBe("high");
  const user = userEvent.setup();
  newInput.focus();
  await user.tab();
  expect(document.activeElement).toBe(newLevels.getByRole("button", { name: "minimal" }));
  await user.keyboard("{Enter}");
  expect(newInput.value).toBe("minimal");
  expect(updates).toEqual([]);
  fireEvent.change(newInput, { target: { value: "custom" } });
  expect(newInput.value).toBe("custom");
});

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
