// 审批档位的共享词汇。
//
// 拼写与 Rust 端 `WorkspaceAccess::wire_name` 一致（下划线），工作区下拉、会话
// 档位面板和接口请求体都用这一套。`read_only` 是工作区策略，不是会话档位。

import type { Messages } from "./i18n";

export type WorkspaceAccess = "read_only" | "strict" | "smart" | "workspace_write" | "full_access";

/** 会话里可选的四档，顺序与 TUI 的 `/permissions` 面板一致。 */
export const selectableAccessModes: WorkspaceAccess[] = ["strict", "smart", "workspace_write", "full_access"];

/** 工作区审批策略的展示名。 */
export function accessLabel(
  access: WorkspaceAccess,
  t: Pick<Messages, "accessReadOnly" | "accessStrict" | "accessSmart" | "accessWrite" | "accessFull">,
): string {
  switch (access) {
    case "read_only": return t.accessReadOnly;
    case "strict": return t.accessStrict;
    case "smart": return t.accessSmart;
    case "workspace_write": return t.accessWrite;
    case "full_access": return t.accessFull;
  }
}

/** 只有完全访问会跳过工作区内的写入与命令审批，面板里要标红并单独确认。 */
export function isDangerousAccess(access: WorkspaceAccess): boolean {
  return access === "full_access";
}
