// 会话审批档位的写入接口。面板按钮与 `/permissions` 命令共用这一条，避免
// 两套「要不要确认」的规则各写一遍。

import { isDangerousAccess, type WorkspaceAccess } from "./accessModes";

/** 服务端拒绝时带回了状态码和一句原因；两者都要保留，才能区分「没权限」和「参数不对」。 */
export class ApprovalModeFailure extends Error {
  constructor(readonly status: number, detail: string) {
    super(detail);
  }
}

export async function changeApprovalMode(sessionId: string, mode: WorkspaceAccess): Promise<void> {
  const response = await fetch(`/api/sessions/${encodeURIComponent(sessionId)}/approval-mode`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    // 升到完全访问必须由用户显式确认；服务端也校验，前端不能替用户点头。
    body: JSON.stringify({ approval_mode: mode, confirm: isDangerousAccess(mode) }),
  });
  if (response.ok) return;
  const body: unknown = await response.json().catch(() => null);
  const detail = body && typeof body === "object" && "error" in body && typeof body.error === "string" ? body.error : "";
  throw new ApprovalModeFailure(response.status, detail || response.statusText);
}
