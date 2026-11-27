// 插件菜单贡献点的非组件部分。
//
// 与 `PluginMenus.tsx` 分开只为一条工程约束：一个文件同时导出组件和普通函数时
// React Fast Refresh 会失效。位置白名单本身在 Rust 侧（`plugin/manifest.rs` 的
// PluginMenuLocation）定义，与 Xedit 共享同一份 schema。

import { useCallback } from "react";
import { executePluginCommand, isPluginRunnable, type CommandResponse, type PluginView } from "./plugins";

export type MenuLocation =
  | "commandPalette"
  | "session.context"
  | "composer.more"
  | "plugin.sidebar.row.context"
  | "chat.selection";

export type MenuEntry = {
  pluginId: string;
  pluginName: string;
  commandId: string;
  title: string;
  icon: string | null;
};

/** 某个位置上，所有可运行插件贡献的命令，按插件安装顺序排列。 */
export function menuEntries(plugins: PluginView[], location: MenuLocation): MenuEntry[] {
  return plugins
    .filter(isPluginRunnable)
    .flatMap((plugin) =>
      (plugin.menus[location] ?? []).flatMap((commandId) => {
        const command = plugin.commands.find((item) => item.id === commandId);
        return command
          ? [{ pluginId: plugin.id, pluginName: plugin.name, commandId, title: command.title, icon: command.icon }]
          : [];
      })
    );
}

/** 执行一条菜单命令，并把跳转类结果交给调用方消化。 */
export function usePluginCommandRunner(onNavigate: (qualifiedId: string) => void) {
  return useCallback(
    async (entry: MenuEntry, args: Record<string, string> = {}): Promise<CommandResponse> => {
      const response = await executePluginCommand(entry.pluginId, entry.commandId, args);
      if (response.destination) onNavigate(response.destination);
      return response;
    },
    [onNavigate]
  );
}
