// @vitest-environment jsdom
import { afterEach, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { ChakraProvider, defaultSystem } from "@chakra-ui/react";
import { PluginRail, type RailSelection } from "./PluginRail";
import { messages, type Language } from "./i18n";
import { agentText } from "./agentLibraryMessages";
import { MAX_PINNED_DESTINATIONS, type PluginDestinationView, type PluginView } from "./plugins";
afterEach(cleanup);
it.each<Language>(["zh-CN", "en", "ja"])("exposes separate agent library and experiments navigation in %s", language => {
  const select=vi.fn();
  render(<ChakraProvider value={defaultSystem}><PluginRail entries={[]} selection={{kind:"agent-experiments"}} messages={messages[language]} schedulesLabel="Fixture schedule" brandLabel={agentText("brand",language)} agentsLabel={agentText("title",language)} experimentsLabel={agentText("experimentTitle",language)} agentsGroupLabel={agentText("navigationGroup",language)} onSelect={select}/></ChakraProvider>);
  const experiment=screen.getByRole("button",{name:agentText("experimentTitle",language)});
  expect(experiment.getAttribute("aria-current")).toBe("page");
  fireEvent.click(screen.getByRole("button",{name:agentText("title",language)}));
  expect(select).toHaveBeenLastCalledWith({kind:"agents"});
  fireEvent.click(experiment);
  expect(select).toHaveBeenLastCalledWith({kind:"agent-experiments"});
  expect(screen.getByRole("img", {name:agentText("brand",language)})).toBeTruthy();
});

it("keeps overflow plugin destinations selectable outside the scrolling rail", () => {
  const select = vi.fn();
  const plugin: PluginView = { id:"fixture", name:"Fixture", version:"1.0.0", description:null, source:"fixture", enabled:true,
    approval_gap:null, permissions:[], inferred_permissions:[], mcp_servers:[], destinations:[], commands:[], menus:{}, settings:[], unsupported:[], file_pickers:[] };
  const entries = Array.from({length:MAX_PINNED_DESTINATIONS + 1}, (_, index) => {
    const destination: PluginDestinationView = { id:String(index), qualified_id:`fixture:${index}`, title:`Fixture ${index}`, icon:null,
      main_page:"fixture", page_runtime:"localWeb", page_url:null, page_server:null, sidebar:null, toolbar_commands:[], default_pinned:true, pinned_order:index };
    return {plugin, destination};
  });
  const last = entries.at(-1)!.destination;
  render(<ChakraProvider value={defaultSystem}><PluginRail entries={entries} selection={{kind:"plugin", qualifiedId:last.qualified_id}} messages={messages["zh-CN"]} schedulesLabel="定时任务" onSelect={select} /></ChakraProvider>);
  fireEvent.click(screen.getByRole("button", {name:messages["zh-CN"].pluginMore}));
  const menu = screen.getByRole("menu");
  expect(menu.closest("nav")).toBeNull();
  fireEvent.click(screen.getByRole("menuitem", {name:last.title}));
  expect(select).toHaveBeenLastCalledWith({kind:"plugin", qualifiedId:last.qualified_id});
  expect(screen.queryByRole("menu")).toBeNull();
});

it("keeps the same navigation structure and order across every top-level page", () => {
  const props = { entries: [], messages: messages["zh-CN"], schedulesLabel: "定时任务", brandLabel: "WillDeep",
    agentsLabel: "智能体库", experimentsLabel: "RSI 实验", agentsGroupLabel: "智能体", onSelect: vi.fn() };
  const selections: RailSelection[] = [{kind:"conversation"}, {kind:"schedules"}, {kind:"agents"}, {kind:"agent-experiments"}, {kind:"configuration"}, {kind:"center"}];
  const view = render(<ChakraProvider value={defaultSystem}><PluginRail {...props} selection={selections[0]} /></ChakraProvider>);
  for (const selection of selections) {
    view.rerender(<ChakraProvider value={defaultSystem}><PluginRail {...props} selection={selection} /></ChakraProvider>);
    const nav = screen.getByRole("navigation");
    expect(nav.classList.contains("plugin-rail")).toBe(true);
    expect(nav.classList.contains("agent-workspace-rail")).toBe(false);
    expect(screen.getAllByRole("button").map(button => button.getAttribute("aria-label"))).toEqual(["对话", "定时任务", "智能体库", "RSI 实验", messages["zh-CN"].configEntry, "插件中心"]);
    expect(nav.querySelectorAll('[aria-current="page"]')).toHaveLength(1);
    expect(screen.getByRole("img", {name:"WillDeep"})).toBeTruthy();
  }
});
