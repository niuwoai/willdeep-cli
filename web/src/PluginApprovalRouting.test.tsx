// @vitest-environment jsdom
import { ChakraProvider, defaultSystem } from "@chakra-ui/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import { App } from "./App";
import { messages } from "./i18n";
import { PluginCommandPalette } from "./PluginMenus";
import { menuEntries, type MenuLocation } from "./pluginMenuModel";
import type { PluginCommandView, PluginView } from "./plugins";

const command: PluginCommandView = { id: "showDetails", title: "Fixture command", icon: null, handler: "host" };
const menuLocations: MenuLocation[] = ["commandPalette", "session.context", "composer.more", "plugin.sidebar.row.context", "chat.selection"];
const plugin: PluginView = {
  id: "approval-fixture", name: "Approval fixture", version: "1.0.0", description: null, source: "shared", enabled: true,
  approval_gap: null, permissions: ["workspace.read"], inferred_permissions: [], mcp_servers: [], settings: [], unsupported: [], file_pickers: [],
  destinations: [{
    id: "home", qualified_id: "approval-fixture:home", title: "Fixture destination", icon: null,
    main_page: "home", page_runtime: "localWeb", page_url: "/plugin-host/approval-fixture/home.html", page_server: null,
    sidebar: { id: "side", mode: "declarative" }, toolbar_commands: [command], default_pinned: true, pinned_order: null,
  }],
  commands: [command], menus: Object.fromEntries(menuLocations.map((location) => [location, [command.id]])),
};
const upgraded: PluginView = { ...plugin, version: "2.0.0", approval_gap: { reason: "version-changed", detail: null } };
const destination = plugin.destinations[0];
const messagesEn = messages.en;

it.each(menuLocations)("only shows commands for runnable plugins in %s", (location) => {
  const disabled = { ...plugin, id: "disabled-fixture", enabled: false };
  const runnable = { ...plugin, id: "other-fixture" };
  expect(menuEntries([upgraded, disabled, runnable], location).map(({ pluginId }) => pluginId)).toEqual([runnable.id]);
});

afterEach(() => {
  cleanup();
  window.history.replaceState(null, "", "/");
  localStorage.clear();
  vi.unstubAllGlobals();
});

it("does not execute a different palette command when the selected plugin loses approval", () => {
  const other = { ...plugin, id: "other-fixture" };
  const entries = menuEntries([plugin, other], "commandPalette");
  const onRun = vi.fn();
  const onClose = vi.fn();
  const palette = (current: typeof entries) => (
    <ChakraProvider value={defaultSystem}>
      <PluginCommandPalette entries={current} messages={messagesEn} onRun={onRun} onClose={onClose} />
    </ChakraProvider>
  );
  const { rerender } = render(palette(entries));
  const input = screen.getByRole("textbox", { name: messagesEn.pluginPalette });
  rerender(palette(menuEntries([upgraded, other], "commandPalette")));
  fireEvent.keyDown(input, { key: "Enter" });
  expect(onRun).not.toHaveBeenCalled();
  fireEvent.keyDown(input, { key: "ArrowDown" });
  fireEvent.keyDown(input, { key: "Enter" });
  expect(onRun).toHaveBeenCalledWith(entries[1]);
});

it("unmounts an open plugin on upgrade and restores its entry after approval in Plugin Center", async () => {
  localStorage.setItem("willdeep.language", "en");
  window.history.replaceState(null, "", `/#plugin/${encodeURIComponent(destination.qualified_id)}`);
  let current = plugin;
  const approvePath = `/api/plugins/${plugin.id}/approve`;
  const fetchMock = vi.fn(async (path: string, init?: RequestInit) => {
    if (path === approvePath && init?.method === "POST") {
      current = { ...upgraded, approval_gap: null };
      return Response.json({});
    }
    if (path === "/api/plugins?locale=en") return Response.json({ plugins: [current], failures: [] });
    if (path === `/api/plugins/${plugin.id}/sidebars/side?locale=en`) {
      return Response.json({ document: { schemaVersion: 1, components: [{ id: "sidebar-label", kind: "label", value: "Fixture sidebar" }] }, degraded: null, strings: {} });
    }
    if (path === "/api/setup") return Response.json({ ready: true });
    if (path === "/api/workspaces") return Response.json([]);
    return Response.json({}, { status: 404 });
  });
  vi.stubGlobal("fetch", fetchMock);
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const { container } = render(<QueryClientProvider client={client}><ChakraProvider value={defaultSystem}><App /></ChakraProvider></QueryClientProvider>);

  await waitFor(() => expect(container.querySelector("iframe")?.title).toBe(destination.title));
  expect(await screen.findByText("Fixture sidebar")).toBeTruthy();
  expect(screen.getByRole("button", { name: command.title })).toBeTruthy();

  current = upgraded;
  await act(async () => { await client.invalidateQueries({ queryKey: ["plugins", "en"] }); });
  await waitFor(() => expect(window.location.hash).toBe(""));
  expect(container.querySelector("iframe")).toBeNull();
  expect(screen.queryByText("Fixture sidebar")).toBeNull();
  expect(screen.queryByRole("button", { name: command.title })).toBeNull();
  expect(screen.queryByRole("button", { name: destination.title })).toBeNull();

  fireEvent.click(screen.getByRole("button", { name: messagesEn.pluginSectionTitle }));
  fireEvent.click(screen.getByRole("button", { name: messagesEn.pluginCenter }));
  expect(screen.getByText(messagesEn.pluginGapVersionChanged)).toBeTruthy();
  fireEvent.click(screen.getByRole("button", { name: messagesEn.pluginApprove }));
  expect(await screen.findByRole("button", { name: messagesEn.pluginDisable })).toBeTruthy();
  expect(fetchMock).toHaveBeenCalledWith(approvePath, { method: "POST" });
  fireEvent.click(screen.getByRole("button", { name: destination.title }));
  await waitFor(() => expect(container.querySelector("iframe")?.title).toBe(destination.title));
});
