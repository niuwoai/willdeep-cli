// @vitest-environment jsdom
import { afterEach, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { ChakraProvider, defaultSystem } from "@chakra-ui/react";
import { PluginRail } from "./PluginRail";
import { messages, type Language } from "./i18n";
import { agentText } from "./agentLibraryMessages";
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
  expect(screen.getByText(agentText("brand",language))).toBeTruthy();
});
