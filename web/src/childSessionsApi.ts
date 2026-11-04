export type ChildTaskStatus = "preparing" | "queued" | "running" | "waiting_approval" | "waiting_answer" | "completed" | "partial" | "failed" | "cancelled" | "interrupted" | "blocked";
export type ChildWorktree = {
  root_workspace: string; path: string; branch: string; base_commit: string;
  base: "head" | "snapshot"; parent_dirty: boolean;
  baseline_patch_sha256: string | null; untracked_files: number;
};
export type ChildResult = {
  revision: number; summary: string; report: string | null; report_complete: boolean;
  consumed_at: number | null; superseded_by: string | null;
  workspace?: string | null; worktree?: ChildWorktree | null;
};
export type ChildTask = {
  assignment_id: string; parent_session_id: string; child_session_id: string;
  child_turn_id: string | null; title: string; prompt: string; status: ChildTaskStatus;
  context: "fresh" | "fork"; workspace_mode: "shared_read_only" | "worktree";
  model: string | null; through_turn_id: string | null;
  created_at: number; updated_at: number; result: ChildResult | null; error: string | null;
};
export type StartChildOptions = {
  title: string; prompt: string;
  context?: "fresh" | "fork";
  /** Omit for the latest completed parent Turn; the Web proxy resolves it. */
  throughTurnId?: string | null;
  workspaceMode?: "shared_read_only" | "worktree";
  worktreeBase?: "head" | "snapshot";
  model?: string | null;
};
export type ChildTaskPage = { tasks: ChildTask[]; next_cursor: string | null };
export const childTaskTerminal = (status: ChildTaskStatus) => ["completed", "partial", "failed", "cancelled", "interrupted", "blocked"].includes(status);
export const childTaskRetryable = (task: ChildTask) => ["partial", "failed", "interrupted", "blocked"].includes(task.status) && task.result != null && task.result.superseded_by == null;
export class ChildSessionFailure extends Error {
  constructor(readonly status: number, detail: string) { super(detail); }
}
const base = (parent: string) => `/api/sessions/${encodeURIComponent(parent)}/children`;
async function request<T>(path: string, init?: RequestInit): Promise<T> {
  const response = await fetch(path, init);
  if (!response.ok) {
    const body: unknown = await response.json().catch(() => null);
    const detail = body && typeof body === "object" && "error" in body && typeof body.error === "string" ? body.error : response.statusText;
    throw new ChildSessionFailure(response.status, detail);
  }
  return response.json() as Promise<T>;
}
const post = (path: string, body: object) => request<ChildTask>(path, {
  method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify(body),
});
export function listChildTasks(parent: string, after: string | null, signal: AbortSignal) {
  return request<ChildTaskPage>(`${base(parent)}?limit=50${after ? `&after=${encodeURIComponent(after)}` : ""}`, { signal });
}
export const startChildTask = (parent: string, options: StartChildOptions, requestId: string) => post(base(parent), {
  title: options.title, prompt: options.prompt, request_id: requestId,
  context: options.context ?? "fresh",
  through_turn_id: options.throughTurnId ?? null,
  workspace_mode: options.workspaceMode ?? "shared_read_only",
  worktree_base: options.worktreeBase ?? "head",
  model: options.model ?? null,
});
export const continueChildTask = (parent: string, child: string, prompt: string, requestId: string) => post(`${base(parent)}/${encodeURIComponent(child)}/continue`, { prompt, request_id: requestId });
export const retryChildTask = (parent: string, task: ChildTask, requestId: string) => post(`${base(parent)}/${encodeURIComponent(task.assignment_id)}/retry`, { expected_result_revision: task.result?.revision, request_id: requestId });
export const stopChildTask = (parent: string, assignment: string, requestId: string) => post(`${base(parent)}/${encodeURIComponent(assignment)}/stop`, { request_id: requestId });
