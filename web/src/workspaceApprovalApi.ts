import type { WorkspaceAccess } from "./accessModes";
import { ApprovalModeFailure } from "./approvalModeApi";

export type WorkspaceSummary = {
  id: string;
  path: string;
  name: string;
  active: boolean;
  access: WorkspaceAccess;
  approval_mode_configured: boolean;
};

export const workspaceListKey = ["workspaces"] as const;

async function workspaceRequest<T>(url: string, init?: RequestInit): Promise<T> {
  const response = await fetch(url, init);
  const body: unknown = await response.json().catch(() => null);
  if (!response.ok) {
    const detail = body && typeof body === "object" && "error" in body && typeof body.error === "string" ? body.error : response.statusText;
    throw new ApprovalModeFailure(response.status, detail);
  }
  return body as T;
}

export function listWorkspaces(): Promise<WorkspaceSummary[]> {
  return workspaceRequest("/api/workspaces");
}

export function addWorkspacePath(path: string): Promise<WorkspaceSummary> {
  return workspaceRequest("/api/workspaces", {
    method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ path }),
  });
}

export function saveWorkspaceApproval(path: string, mode: WorkspaceAccess, confirmed: boolean): Promise<WorkspaceSummary> {
  return workspaceRequest("/api/workspaces/approval-mode", {
    method: "POST", headers: { "content-type": "application/json" },
    body: JSON.stringify({ path, approval_mode: mode, confirm: mode === "full_access" && confirmed }),
  });
}
