// @vitest-environment jsdom
import { afterEach, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { ChakraProvider, defaultSystem } from "@chakra-ui/react";
import { PluginRail, type RailSelection } from "./PluginRail";
import { messages, type Language } from "./i18n";
import { agentText } from "./agentLibraryMessages";
import type { PluginDestinationView, PluginView } from "./plugins";
afterEach(cleanup);

it.each<Language>(["zh-CN", "en", "ja"])("keeps configuration destinations under their own first-level entry in %s", language => {
  const select = vi.fn();
  const text = messages[language];
  render(<ChakraProvider value={defaultSystem}><PluginRail
    entries={[]}
    selection={{ kind: "agent-experiments" }}
    messages={text}
    schedulesLabel="Fixture schedule"
    brandLabel={agentText("brand", language)}
    agentsLabel={agentText("title", language)}
    experimentsLabel={agentText("experimentTitle", language)}
    agentsGroupLabel={agentText("navigationGroup", language)}
    onSelect={select}
  /></ChakraProvider>);

  const configuration = screen.getByRole("button", { name: text.configEntry });
  const plugins = screen.getByRole("button", { name: text.pluginSectionTitle });
  expect(configuration.getAttribute("aria-expanded")).toBe("false");
  expect(plugins.getAttribute("aria-expanded")).toBe("false");
  expect(screen.queryByRole("button", { name: text.intakeEntry })).toBeNull();
  expect(screen.getAllByRole("button").map(button => button.getAttribute("aria-label"))).toEqual([
    text.conversation, "Fixture schedule", text.configEntry, text.pluginSectionTitle,
  ]);

  fireEvent.click(configuration);
  expect(configuration.getAttribute("aria-expanded")).toBe("true");
  expect(plugins.getAttribute("aria-expanded")).toBe("false");
  expect(screen.getByRole("button", { name: text.configTitle })).toBeTruthy();
  expect(screen.getByRole("button", { name: text.intakeEntry })).toBeTruthy();
  expect(screen.getByRole("button", { name: text.mcpAccess })).toBeTruthy();
  expect(screen.getByRole("button", { name: agentText("title", language) })).toBeTruthy();
  const experiment = screen.getByRole("button", { name: agentText("experimentTitle", language) });
  expect(experiment.getAttribute("aria-current")).toBe("page");
  expect(screen.queryByRole("button", { name: text.pluginCenter })).toBeNull();
  fireEvent.click(experiment);
  expect(select).toHaveBeenLastCalledWith({ kind: "agent-experiments" });
  expect(screen.queryByRole("group", { name: text.configEntry })).toBeNull();
  expect(screen.getByRole("img", { name: agentText("brand", language) })).toBeTruthy();
});

it("opens all enabled plugin destinations and Plugin Center from the first-level Plugins entry", () => {
  const select = vi.fn();
  const text = messages["zh-CN"];
  const plugin: PluginView = {
    id: "fixture", name: "Fixture", version: "1.0.0", description: null, source: "fixture", enabled: true,
    approval_gap: null, permissions: [], inferred_permissions: [], mcp_servers: [], destinations: [], commands: [], menus: {}, settings: [], unsupported: [], file_pickers: [],
  };
  const entries = Array.from({ length: 6 }, (_, index) => {
    const destination: PluginDestinationView = {
      id: String(index), qualified_id: `fixture:${index}`, title: `Fixture ${index}`, icon: null,
      main_page: "fixture", page_runtime: "localWeb", page_url: null, page_server: null, sidebar: null,
      toolbar_commands: [], default_pinned: true, pinned_order: index,
    };
    return { plugin, destination };
  });
  const last = entries.at(-1)!.destination;
  render(<ChakraProvider value={defaultSystem}><PluginRail entries={entries} selection={{ kind: "conversation" }} messages={text} schedulesLabel="定时任务" onSelect={select} /></ChakraProvider>);

  const configuration = screen.getByRole("button", { name: text.configEntry });
  const plugins = screen.getByRole("button", { name: text.pluginSectionTitle });
  fireEvent.click(plugins);
  expect(plugins.getAttribute("aria-expanded")).toBe("true");
  expect(configuration.getAttribute("aria-expanded")).toBe("false");
  expect(screen.getByRole("button", { name: text.pluginCenter })).toBeTruthy();
  expect(screen.getByRole("button", { name: last.title })).toBeTruthy();
  fireEvent.click(screen.getByRole("button", { name: last.title }));
  expect(select).toHaveBeenLastCalledWith({ kind: "plugin", qualifiedId: last.qualified_id });
  expect(screen.queryByRole("group", { name: text.pluginSectionTitle })).toBeNull();
  expect(plugins.getAttribute("aria-expanded")).toBe("false");
});

it("keeps the same compact top-level navigation across every page", () => {
  const text = messages["zh-CN"];
  const props = {
    entries: [], messages: text, schedulesLabel: "定时任务", brandLabel: "WillDeep",
    agentsLabel: "智能体库", experimentsLabel: "RSI 实验", agentsGroupLabel: "智能体", onSelect: vi.fn(),
  };
  const selections: RailSelection[] = [
    { kind: "conversation" }, { kind: "schedules" }, { kind: "intake" }, { kind: "mcp" },
    { kind: "agents" }, { kind: "agent-experiments" }, { kind: "configuration" }, { kind: "center" },
    { kind: "plugin", qualifiedId: "fixture:home" },
  ];
  const view = render(<ChakraProvider value={defaultSystem}><PluginRail {...props} selection={selections[0]} /></ChakraProvider>);
  for (const selection of selections) {
    view.rerender(<ChakraProvider value={defaultSystem}><PluginRail {...props} selection={selection} /></ChakraProvider>);
    const nav = screen.getByRole("navigation");
    expect(nav.classList.contains("plugin-rail")).toBe(true);
    expect(nav.classList.contains("agent-workspace-rail")).toBe(false);
    expect(screen.getAllByRole("button").map(button => button.getAttribute("aria-label"))).toEqual([
      "对话", "定时任务", text.configEntry, text.pluginSectionTitle,
    ]);
    expect(nav.querySelectorAll('[aria-current="page"]')).toHaveLength(1);
    expect(screen.getByRole("img", { name: "WillDeep" })).toBeTruthy();
  }
});
