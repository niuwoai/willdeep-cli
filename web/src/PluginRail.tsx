// 一级入口栏。获准固定的插件排在定时任务下方，配置与其余插件按需展开。
// 所有选中状态沿用宿主灰度，不使用插件自己的品牌色。

import { Box, Flex, Portal, Text } from "@chakra-ui/react";
import { useCallback, useEffect, useRef, useState, type ReactNode } from "react";
import { FlaskConical, Globe, Library, Plug, Settings2, Users } from "lucide-react";
import brandIcon from "./assets/willdeep-app-icon.png";

import type { Messages } from "./i18n";
import { PluginIcon, SfIcon } from "./sfSymbols";
import { MAX_PINNED_DESTINATIONS, type PluginDestinationView, type PluginView } from "./plugins";

export type RailSelection = { kind: "conversation" } | { kind: "agents" } | { kind: "agent-experiments" } | { kind: "schedules" } | { kind: "intake" } | { kind: "mcp" } | { kind: "configuration" } | { kind: "center" } | { kind: "plugin"; qualifiedId: string };

type Props = {
  entries: Array<{ plugin: PluginView; destination: PluginDestinationView }>;
  selection: RailSelection;
  messages: Messages;
  schedulesLabel: string;
  agentsLabel?: string;
  experimentsLabel?: string;
  agentsGroupLabel?: string;
  brandLabel?: string;
  onSelect: (selection: RailSelection) => void;
};

type OpenMenu = "configuration" | "plugins" | null;
type ButtonOptions = { current?: boolean; expanded?: boolean; controls?: string };

export function PluginRail({ entries, selection, messages, schedulesLabel, agentsLabel, experimentsLabel, agentsGroupLabel, brandLabel, onSelect }: Props) {
  const [openMenu, setOpenMenu] = useState<OpenMenu>(null);
  const configurationTriggerRef = useRef<HTMLDivElement>(null);
  const configurationMenuRef = useRef<HTMLDivElement>(null);
  const pluginsTriggerRef = useRef<HTMLDivElement>(null);
  const pluginsMenuRef = useRef<HTMLDivElement>(null);
  const pinned = entries.filter(({ destination }) => destination.default_pinned || destination.pinned_order !== null).slice(0, MAX_PINNED_DESTINATIONS);
  const pinnedIds = new Set(pinned.map(({ destination }) => destination.qualified_id));
  const menuEntries = entries.filter(({ destination }) => !pinnedIds.has(destination.qualified_id));
  const activeId = selection.kind === "plugin" ? selection.qualifiedId : null;
  const configurationActive = selection.kind === "configuration" || selection.kind === "intake" || selection.kind === "mcp"
    || selection.kind === "agents" || selection.kind === "agent-experiments";
  const pluginsActive = selection.kind === "center" || (selection.kind === "plugin" && !pinnedIds.has(selection.qualifiedId));

  const closeMenus = useCallback(() => setOpenMenu(null), []);

  useEffect(() => {
    if (openMenu === null) return;
    const triggerRef = openMenu === "configuration" ? configurationTriggerRef : pluginsTriggerRef;
    const menuRef = openMenu === "configuration" ? configurationMenuRef : pluginsMenuRef;
    const closeOnOutsideClick = (event: PointerEvent) => {
      const target = event.target;
      if (!(target instanceof Node)) return;
      if (triggerRef.current?.contains(target) || menuRef.current?.contains(target)) return;
      closeMenus();
    };
    const closeOnEscape = (event: KeyboardEvent) => {
      if (event.key === "Escape") closeMenus();
    };
    document.addEventListener("pointerdown", closeOnOutsideClick);
    document.addEventListener("keydown", closeOnEscape);
    return () => {
      document.removeEventListener("pointerdown", closeOnOutsideClick);
      document.removeEventListener("keydown", closeOnEscape);
    };
  }, [openMenu, closeMenus]);

  const button = (key: string, label: string, icon: ReactNode, active: boolean, onClick: () => void, options: ButtonOptions = {}) => (
    <button
      key={key}
      type="button"
      className={`rail-button${active ? " active" : ""}`}
      title={label}
      aria-label={label}
      aria-current={(options.current ?? active) ? "page" : undefined}
      aria-expanded={options.expanded}
      aria-controls={options.controls}
      onClick={onClick}
    >
      {icon}
      <Text className="rail-label">{label}</Text>
    </button>
  );

  const menuItem = (key: string, label: string, icon: ReactNode, active: boolean, onClick: () => void) => (
    <button
      key={key}
      type="button"
      className={`rail-menu-item${active ? " active" : ""}`}
      aria-current={active ? "page" : undefined}
      onClick={() => {
        closeMenus();
        onClick();
      }}
    >
      {icon}
      <span>{label}</span>
    </button>
  );

  const toggleMenu = (menu: Exclude<OpenMenu, null>) => {
    setOpenMenu((current) => current === menu ? null : menu);
  };

  return (
    <Flex as="nav" className="plugin-rail" direction="column" aria-label={messages.pluginNavigation}>
      <div className="rail-brand" title={brandLabel}><img src={brandIcon} alt={brandLabel ?? messages.appName} /></div>
      <div className="rail-scroll">
        {button(
          "conversation",
          messages.conversation,
          <SfIcon name="sf:text.badge.plus" size={20} />,
          selection.kind === "conversation",
          () => onSelect({ kind: "conversation" })
        )}
        {button("schedules", schedulesLabel, <SfIcon name="sf:clock" size={20} />, selection.kind === "schedules", () => onSelect({ kind: "schedules" }))}
        {pinned.map(({ plugin, destination }) => button(
          destination.qualified_id,
          destination.title,
          <PluginIcon pluginId={plugin.id} icon={destination.icon} size={20} />,
          activeId === destination.qualified_id,
          () => { closeMenus(); onSelect({ kind: "plugin", qualifiedId: destination.qualified_id }); }
        ))}
      </div>
      <Box className="rail-config-anchor" ref={configurationTriggerRef}>
        {button(
          "configuration",
          messages.configEntry,
          <Settings2 size={20} />,
          configurationActive,
          () => toggleMenu("configuration"),
          { current: configurationActive && openMenu !== "configuration", expanded: openMenu === "configuration", controls: "rail-configuration-menu" }
        )}
      </Box>
      <Box className="rail-plugins-anchor" ref={pluginsTriggerRef}>
        {button(
          "plugins",
          messages.pluginSectionTitle,
          <Plug size={20} />,
          pluginsActive,
          () => toggleMenu("plugins"),
          { current: pluginsActive && openMenu !== "plugins", expanded: openMenu === "plugins", controls: "rail-plugin-menu" }
        )}
      </Box>
      {openMenu === "configuration" && (
        <Portal>
          <Box id="rail-configuration-menu" ref={configurationMenuRef} className="rail-configuration-menu" role="group" aria-label={messages.configEntry}>
            <Text className="rail-configuration-heading">{messages.configEntry}</Text>
            {menuItem("configuration-page", messages.configTitle, <Settings2 size={16} />, selection.kind === "configuration", () => onSelect({ kind: "configuration" }))}
            {menuItem("intake", messages.intakeEntry, <Globe size={16} />, selection.kind === "intake", () => onSelect({ kind: "intake" }))}
            {menuItem("mcp", messages.mcpAccess, <Plug size={16} />, selection.kind === "mcp", () => onSelect({ kind: "mcp" }))}
            {(agentsLabel || experimentsLabel) && <>
              <Box className="rail-menu-divider" />
              {agentsGroupLabel && <Text className="rail-menu-group-label"><Users size={14} />{agentsGroupLabel}</Text>}
              {agentsLabel && menuItem("agents", agentsLabel, <Library size={16} />, selection.kind === "agents", () => onSelect({ kind: "agents" }))}
              {experimentsLabel && menuItem("agent-experiments", experimentsLabel, <FlaskConical size={16} />, selection.kind === "agent-experiments", () => onSelect({ kind: "agent-experiments" }))}
            </>}
          </Box>
        </Portal>
      )}
      {openMenu === "plugins" && (
        <Portal>
          <Box id="rail-plugin-menu" ref={pluginsMenuRef} className="rail-plugin-menu" role="group" aria-label={messages.pluginSectionTitle}>
            <Text className="rail-configuration-heading">{messages.pluginSectionTitle}</Text>
            {menuItem("plugin-center", messages.pluginCenter, <SfIcon name="sf:gearshape" size={16} />, selection.kind === "center", () => onSelect({ kind: "center" }))}
            {menuEntries.length > 0 && <Box className="rail-menu-divider" />}
            {menuEntries.map(({ plugin, destination }) => menuItem(
              destination.qualified_id,
              destination.title,
              <PluginIcon pluginId={plugin.id} icon={destination.icon} size={14} />,
              activeId === destination.qualified_id,
              () => onSelect({ kind: "plugin", qualifiedId: destination.qualified_id })
            ))}
          </Box>
        </Portal>
      )}
    </Flex>
  );
}
