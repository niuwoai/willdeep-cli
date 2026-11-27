// 插件菜单贡献点的界面部分：命令面板、右键/气泡浮层。
//
// 目的地是插件的主入口，菜单是它的**顺手入口**——收藏夹和待办真正的用法是
// 「聊天里选中一句就能记下」，而不是先切到那个目的地再手打一遍。所以这几个
// 位置不是锦上添花，是那两个插件的一半价值所在。
//
// 非组件的部分（位置类型、命令收集、选中监听）在 `pluginMenuModel.ts`。

import { useEffect, useMemo, useRef, useState } from "react";
import { Box, Text } from "@chakra-ui/react";
import type { Messages } from "./i18n";
import { PluginIcon } from "./sfSymbols";
import type { MenuEntry } from "./pluginMenuModel";

type PaletteProps = {
  entries: MenuEntry[];
  messages: Messages;
  onRun: (entry: MenuEntry) => void;
  onClose: () => void;
};

function entryKey(entry: MenuEntry) {
  return JSON.stringify([entry.pluginId, entry.commandId]);
}

function matchesQuery(entry: MenuEntry, query: string) {
  return `${entry.title} ${entry.pluginName} ${entry.commandId}`.toLowerCase().includes(query.trim().toLowerCase());
}

export function PluginCommandPalette({ entries, messages, onRun, onClose }: PaletteProps) {
  const [query, setQuery] = useState("");
  const [activeKey, setActiveKey] = useState<string | null>(() => entries[0] ? entryKey(entries[0]) : null);
  const inputRef = useRef<HTMLInputElement | null>(null);
  const matches = useMemo(() => entries.filter((entry) => matchesQuery(entry, query)), [entries, query]);
  const activeIndex = matches.findIndex((entry) => entryKey(entry) === activeKey);

  useEffect(() => {
    inputRef.current?.focus();
  }, []);

  function select(entry: MenuEntry | undefined) {
    setActiveKey(entry ? entryKey(entry) : null);
  }

  return (
    <Box className="plugin-palette-backdrop" onMouseDown={onClose}>
      <Box className="plugin-palette" onMouseDown={(event) => event.stopPropagation()}>
        <input
          ref={inputRef}
          className="plugin-palette-input"
          value={query}
          placeholder={messages.pluginPalettePlaceholder}
          aria-label={messages.pluginPalette}
          onChange={(event) => {
            const nextQuery = event.target.value;
            setQuery(nextQuery);
            select(entries.find((entry) => matchesQuery(entry, nextQuery)));
          }}
          onKeyDown={(event) => {
            if (event.key === "Escape") { onClose(); return; }
            if (event.key === "ArrowDown" || event.key === "ArrowUp") {
              event.preventDefault();
              const nextIndex = activeIndex + (event.key === "ArrowDown" ? 1 : -1);
              select(matches[Math.max(0, Math.min(nextIndex, matches.length - 1))]);
            }
            if (event.key === "Enter" && matches[activeIndex]) {
              event.preventDefault();
              onRun(matches[activeIndex]);
            }
          }}
        />
        <Box className="plugin-palette-list">
          {matches.map((entry, index) => (
            <button
              key={entryKey(entry)}
              type="button"
              className={`plugin-palette-item${index === activeIndex ? " active" : ""}`}
              onMouseEnter={() => select(entry)}
              onClick={() => onRun(entry)}
            >
              <PluginIcon pluginId={entry.pluginId} icon={entry.icon} kind="command" size={15} />
              <span className="plugin-palette-title">{entry.title}</span>
              <span className="plugin-palette-source">{entry.pluginName}</span>
            </button>
          ))}
          {matches.length === 0 && <Text className="plugin-palette-empty">{messages.pluginPaletteEmpty}</Text>}
        </Box>
      </Box>
    </Box>
  );
}

type PopupProps = {
  entries: MenuEntry[];
  x: number;
  y: number;
  onRun: (entry: MenuEntry) => void;
  onClose: () => void;
  selectionActions?: {
    label: string;
    focusFirst?: boolean;
    restoreFocus: () => void;
    buttons: { label: string; icon: React.ReactNode; run: () => void }[];
  };
};

/** 右键菜单与选中气泡共用的浮层。定位会夹到视口内，免得贴边时半截在屏幕外。 */
export function PluginMenuPopup({ entries, x, y, onRun, onClose, selectionActions }: PopupProps) {
  const ref = useRef<HTMLDivElement | null>(null);
  const [position, setPosition] = useState({ left: x, top: y });

  useEffect(() => {
    const node = ref.current;
    if (!node) return;
    const rect = node.getBoundingClientRect();
    setPosition({
      left: Math.max(8, Math.min(x, window.innerWidth - rect.width - 8)),
      top: Math.max(8, Math.min(y, window.innerHeight - rect.height - 8)),
    });
  }, [x, y, entries.length]);

  const focusFirst = selectionActions?.focusFirst;
  useEffect(() => {
    if (focusFirst) ref.current?.querySelector("button")?.focus({ preventScroll: true });
  }, [focusFirst]);

  // onClose 在 App 里是内联箭头函数，每次渲染都是新的。存进 ref，
  // 免得下面那个 effect 每渲染一次就重新注册一轮监听。
  const closeRef = useRef(onClose);
  closeRef.current = onClose;

  useEffect(() => {
    const dismiss = (event: PointerEvent) => {
      if (!ref.current?.contains(event.target as Node)) closeRef.current();
    };
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape" && !event.defaultPrevented) closeRef.current();
    };
    // 菜单由 pointerup/contextmenu 打开；只在下一次外部按下时关闭，
    // 避免同一次选中/右键手势末尾的 click 将它立即关掉。
    window.addEventListener("pointerdown", dismiss);
    window.addEventListener("keydown", onKey);
    return () => {
      window.removeEventListener("pointerdown", dismiss);
      window.removeEventListener("keydown", onKey);
    };
  }, []);

  if (entries.length === 0 && !selectionActions) return null;

  return (
    <Box ref={ref} className="plugin-popup" style={{ left: position.left, top: position.top }}
      role={selectionActions ? "group" : "menu"} aria-label={selectionActions?.label}
      data-chat-selection-actions={selectionActions ? "" : undefined}
      onMouseDown={(event) => event.preventDefault()} onContextMenu={(event) => event.preventDefault()}
      onKeyDown={(event) => {
        if (selectionActions && event.key === "Escape") {
          event.preventDefault();
          selectionActions.restoreFocus();
          onClose();
        }
      }}>
      {selectionActions?.buttons.map(({ label, icon, run }) => (
        <button key={label} type="button" className="plugin-popup-item" onClick={run}>{icon}<span>{label}</span></button>
      ))}
      {entries.map((entry) => (
        <button
          key={`${entry.pluginId}:${entry.commandId}`}
          type="button"
          role={selectionActions ? undefined : "menuitem"}
          className="plugin-popup-item"
          onClick={() => onRun(entry)}
        >
          <PluginIcon pluginId={entry.pluginId} icon={entry.icon} kind="command" size={14} />
          <span>{entry.title}</span>
          <small>{entry.pluginName}</small>
        </button>
      ))}
    </Box>
  );
}
