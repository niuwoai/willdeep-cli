// Development-only navigation fixture: real components with synthetic plugin data, no daemon.
import React, { useState } from "react";
import { createRoot } from "react-dom/client";
import { Box, ChakraProvider, defaultSystem, Flex, Heading } from "@chakra-ui/react";
import { PluginRail, type RailSelection } from "./src/PluginRail";
import { PluginCenter } from "./src/PluginCenter";
import { orderedDestinations, type PluginView } from "./src/plugins";
import { messages, type Language } from "./src/i18n";
import { scheduleText } from "./src/scheduleMessages";
import "./src/theme.css";
import "./src/styles.css";
import "./src/plugins.css";

if (!import.meta.env.DEV) throw new Error("Plugin navigation fixture is development only");
const params = new URLSearchParams(location.search);
const language: Language = params.get("lang") === "en" ? "en" : params.get("lang") === "ja" ? "ja" : "zh-CN";
const text = messages[language];
document.documentElement.lang = language;
document.documentElement.dataset.theme = params.get("theme") === "dark" ? "dark" : "light";
const names = {
  "zh-CN": ["收藏夹", "待办", "短剧工坊", "蜂巢防线", "会话体检中心", "诊断探针", "其他插件"],
  en: ["Favorites", "Todo", "Screenplay", "Hive Defense", "Session Health", "Diagnostics", "Other plugin"],
  ja: ["お気に入り", "タスク", "脚本工房", "ハイブ防衛", "会話診断", "診断プローブ", "その他のプラグイン"],
};
const definitions = [
  ["favorites", "sf:bookmark", true, null], ["todo", "sf:checklist", false, 0],
  ["screenplay", "sf:film", true, null], ["hive", "sf:hexagon.fill", true, null],
  ["health", "sf:waveform.path.ecg", true, null], ["diagnostics", "sf:wrench", false, null],
  ["overflow", "sf:puzzlepiece", false, 1],
] as const;
const plugins: PluginView[] = params.get("scenario") === "empty" ? [] : definitions.map(([id, icon, defaultPinned, order], index) => ({
  id, name: names[language][index], version: "1.0.0", description: null, source: "shared", enabled: true,
  approval_gap: null, permissions: [], inferred_permissions: [], mcp_servers: [], commands: [], menus: {}, settings: [], unsupported: [], file_pickers: [],
  destinations: [{ id: "home", qualified_id: `${id}:home`, title: names[language][index], icon, main_page: "home", page_runtime: "declarative", page_url: null, page_server: null, sidebar: null, toolbar_commands: [], default_pinned: defaultPinned, pinned_order: params.get("scenario") === "unpin" && id === "todo" ? null : order }],
}));
// Any accidental action in PluginCenter fails locally; never forward to a real service.
globalThis.fetch = async () => new Response(JSON.stringify({ error: "Isolated navigation fixture" }), { status: 400, headers: { "Content-Type": "application/json" } });

export function Preview() {
  const initialId = location.hash.startsWith("#plugin/") ? decodeURIComponent(location.hash.slice(8)) : null;
  const [selection, setSelection] = useState<RailSelection>(initialId ? { kind: "plugin", qualifiedId: initialId } : { kind: "center" });
  const entries = orderedDestinations(plugins);
  const active = selection.kind === "plugin" ? entries.find(({ destination }) => destination.qualified_id === selection.qualifiedId) : undefined;
  const navigate = (next: RailSelection) => {
    setSelection(next);
    history.replaceState(null, "", next.kind === "plugin" ? `#plugin/${encodeURIComponent(next.qualifiedId)}` : `#${next.kind}`);
  };
  const title = active?.destination.title ?? (selection.kind === "schedules" ? scheduleText("title", language) : text.conversation);
  return <Flex minH="100vh" bg="var(--bg-page)" color="var(--text)">
    <PluginRail entries={entries} selection={selection} messages={text} schedulesLabel={scheduleText("title", language)} onSelect={navigate} />
    <Box as="main" flex="1" minW="0" data-selected-destination={active?.destination.qualified_id}>
      {selection.kind === "center" ? <PluginCenter plugins={plugins} failures={[]} messages={text} onChanged={() => undefined} /> : <Heading p="8" size="lg">{title}</Heading>}
    </Box>
  </Flex>;
}
createRoot(document.getElementById("root")!).render(<ChakraProvider value={defaultSystem}><Preview /></ChakraProvider>);
