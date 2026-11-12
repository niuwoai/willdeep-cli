// @vitest-environment jsdom
import { afterEach, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { ChakraProvider, defaultSystem } from "@chakra-ui/react";
import { PluginRail, type RailSelection } from "./PluginRail";
import { messages, type Language } from "./i18n";
import { agentText } from "./agentLibraryMessages";
import { orderedDestinations, type PluginDestinationView, type PluginView } from "./plugins";
afterEach(cleanup);

function fixturePlugin(id: string, defaultPinned: boolean, pinnedOrder: number | null = null): PluginView {
  return {
    id, name: id, version: "1.0.0", description: null, source: "fixture", enabled: true,
    approval_gap: null, permissions: [], inferred_permissions: [], mcp_servers: [], commands: [], menus: {}, settings: [], unsupported: [], file_pickers: [],
    destinations: [{
      id: "home", qualified_id: `${id}:home`, title: id, icon: null,
      main_page: "home", page_runtime: "localWeb", page_url: null, page_server: null, sidebar: null,
      toolbar_commands: [], default_pinned: defaultPinned, pinned_order: pinnedOrder,
    }],
  };
}

it("pins default and explicitly pinned destinations, including order zero, without pinning ordinary entries", () => {
  const text = messages["zh-CN"];
  const select = vi.fn();
  const plugins = [fixturePlugin("Ordinary", false), fixturePlugin("Manual", false, 0), fixturePlugin("Default", true), { ...fixturePlugin("Disabled", true), enabled: false }];
  const renderRail = (items: PluginView[], selection: RailSelection) => <ChakraProvider value={defaultSystem}><PluginRail
    entries={orderedDestinations(items)} selection={selection} messages={text} schedulesLabel="定时任务" onSelect={select}
  /></ChakraProvider>;
  const view = render(renderRail(plugins, { kind: "plugin", qualifiedId: "Manual:home" }));
  const nav = screen.getByRole("navigation");
  expect(within(nav).getAllByRole("button").map(button => button.getAttribute("aria-label"))).toEqual([
    text.conversation, "定时任务", text.federationEntry, "Default", "Manual", text.configEntry, text.pluginSectionTitle,
  ]);
  expect(nav.querySelectorAll('[aria-current="page"]')).toHaveLength(1);
  expect(screen.getByRole("button", { name: "Manual" }).getAttribute("aria-current")).toBe("page");
  const trigger = screen.getByRole("button", { name: text.pluginSectionTitle });
  expect(trigger.classList.contains("active")).toBe(false);
  fireEvent.click(trigger);
  expect(screen.getByRole("button", { name: "Ordinary" })).toBeTruthy();
  fireEvent.click(screen.getByRole("button", { name: "Manual" }));
  expect(select).toHaveBeenLastCalledWith({ kind: "plugin", qualifiedId: "Manual:home" });
  expect(screen.queryByRole("group", { name: text.pluginSectionTitle })).toBeNull();

  const unpinned = plugins.map(plugin => plugin.id === "Manual" ? fixturePlugin("Manual", false) : plugin);
  view.rerender(renderRail(unpinned, { kind: "plugin", qualifiedId: "Manual:home" }));
  expect(within(nav).queryByRole("button", { name: "Manual" })).toBeNull();
  expect(trigger.getAttribute("aria-current")).toBe("page");
  fireEvent.click(trigger);
  const menu = screen.getByRole("group", { name: text.pluginSectionTitle });
  expect(within(menu).getByRole("button", { name: "Manual" }).getAttribute("aria-current")).toBe("page");
  fireEvent.click(within(menu).getByRole("button", { name: "Manual" }));
  expect(select).toHaveBeenLastCalledWith({ kind: "plugin", qualifiedId: "Manual:home" });
  expect(screen.queryByRole("group", { name: text.pluginSectionTitle })).toBeNull();
});

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
    text.conversation, "Fixture schedule", text.federationEntry, text.configEntry, text.pluginSectionTitle,
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

it("places pinned destinations below schedules and keeps overflow in the Plugins menu", () => {
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
  expect(within(screen.getByRole("navigation")).getAllByRole("button").map(button => button.getAttribute("aria-label"))).toEqual([
    text.conversation, "定时任务", text.federationEntry, ...entries.slice(0, 5).map(({ destination }) => destination.title), text.configEntry, text.pluginSectionTitle,
  ]);
  expect(screen.queryByRole("button", { name: last.title })).toBeNull();
  fireEvent.click(plugins);
  expect(plugins.getAttribute("aria-expanded")).toBe("true");
  expect(configuration.getAttribute("aria-expanded")).toBe("false");
  expect(screen.getByRole("button", { name: text.pluginCenter })).toBeTruthy();
  const menu = screen.getByRole("group", { name: text.pluginSectionTitle });
  expect(within(menu).queryByRole("button", { name: entries[0].destination.title })).toBeNull();
  expect(within(menu).getAllByRole("button").map(button => button.textContent)).toEqual([text.pluginCenter, last.title]);
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
      "对话", "定时任务", "联邦", text.configEntry, text.pluginSectionTitle,
    ]);
    expect(nav.querySelectorAll('[aria-current="page"]')).toHaveLength(1);
    expect(screen.getByRole("img", { name: "WillDeep" })).toBeTruthy();
  }
});
