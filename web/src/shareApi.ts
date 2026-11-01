// 会话分享的接口层。token 由服务端生成；完整链接要配合公网入口的实际地址
// 由前端拼出来（入口与 Web 是两个监听）。

import { intakeBase, type IntakeView } from "./intakeApi";

export type ShareMode = "read" | "chat";

export type ShareView = {
  token: string;
  mode: ShareMode;
  created_at: number;
  /// 入口监听上的路径，如 `/share/abc…`。
  path: string;
};

export type ShareResponse = { share: ShareView | null };

export class ShareFailure extends Error {
  constructor(readonly status: number, detail: string) {
    super(detail);
  }
}

async function failure(response: Response): Promise<ShareFailure> {
  const body: unknown = await response.json().catch(() => null);
  const detail = body && typeof body === "object" && "error" in body && typeof body.error === "string" ? body.error : "";
  return new ShareFailure(response.status, detail || response.statusText);
}

export async function loadShare(sessionId: string, signal?: AbortSignal): Promise<ShareResponse> {
  const response = await fetch(`/api/sessions/${encodeURIComponent(sessionId)}/share`, { signal });
  if (!response.ok) throw await failure(response);
  return await response.json() as ShareResponse;
}

export async function createShare(sessionId: string, mode: ShareMode): Promise<ShareResponse> {
  const response = await fetch(`/api/sessions/${encodeURIComponent(sessionId)}/share`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ mode }),
  });
  if (!response.ok) throw await failure(response);
  return await response.json() as ShareResponse;
}

export async function removeShare(sessionId: string): Promise<ShareResponse> {
  const response = await fetch(`/api/sessions/${encodeURIComponent(sessionId)}/share`, { method: "DELETE" });
  if (!response.ok) throw await failure(response);
  return await response.json() as ShareResponse;
}

/// 分享链接的完整地址。入口没在跑时返回 `null`——链接存在但打不开，
/// 页面要如实说这一层，而不是给一个点不动的 URL。
export function shareUrl(share: ShareView, intake: Pick<IntakeView, "running" | "running_listen" | "listen"> | null): string | null {
  if (!intake) return null;
  const base = intakeBase(intake);
  return base ? `${base}${share.path}` : null;
}
