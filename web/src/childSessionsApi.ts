export type ChildTaskStatus = "preparing" | "queued" | "running" | "waiting_approval" | "waiting_answer" | "completed" | "partial" | "failed" | "cancelled" | "interrupted" | "blocked";
export type ChildControlState = "delegated" | "user_owned" | "detached";
export type ChildSubscription = "none" | "inbox" | "auto_wake";
export type ChildLink = {
  parent_session_id: string; child_session_id: string;
  control_state: ChildControlState; control_revision: number; control_updated_at?: number;
};
export type ChildCapabilities = { capabilities: string[]; operations?: string[] };
export const CHILD_CAPABILITIES = {
  control: "session_children_control", subscriptions: "session_children_subscriptions", dependencies: "session_children_dependencies",
} as const;
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
  /** Optional only for older hosts. These are projections of the current link. */
  control_state?: ChildControlState; control_revision?: number;
  subscription?: ChildSubscription; subscription_revision?: number;
  notify?: "none" | "on_task_terminal";
  depends_on?: string[]; waiting_on_dependencies?: boolean;
};
export type ChildTaskOptions = { subscription?: ChildSubscription; dependsOn?: string[] };
export type StartChildOptions = ChildTaskOptions & {
  title: string; prompt: string;
  context?: "fresh" | "fork";
  /** Omit for the latest completed parent Turn; the Web proxy resolves it. */
  throughTurnId?: string | null;
  workspaceMode?: "shared_read_only" | "worktree";
  worktreeBase?: "head" | "snapshot";
  model?: string | null;
};
export type ChildTaskPage = { tasks: ChildTask[]; next_cursor: string | null };
export type ChildProposal = {
  id: string; created_at: number; dismissed: boolean; error?: string | null;
  params: {
    parent_session_id: string; created_from_turn_id?: string | null;
    title: string; prompt: string; context: "fresh" | "fork";
    through_turn_id?: string | null; workspace_mode: "shared_read_only" | "worktree";
    worktree_base: "head" | "snapshot"; model?: string | null;
    notify: "none" | "on_task_terminal"; subscription?: ChildSubscription | null;
    depends_on?: string[];
  };
};
export type ChildProposalPage = { proposals: ChildProposal[] };
export const childTaskTerminal = (status: ChildTaskStatus) => ["completed", "partial", "failed", "cancelled", "interrupted", "blocked"].includes(status);
export const childTaskRetryable = (task: ChildTask) => ["partial", "failed", "interrupted", "blocked"].includes(task.status) && task.result != null && task.result.superseded_by == null;
export const childTaskControlled = (task: ChildTask) => (task.control_state ?? "delegated") === "delegated";
export const childTaskSubscription = (task: ChildTask): ChildSubscription => task.subscription ?? (task.notify === "none" ? "none" : "auto_wake");
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
const post = <T = ChildTask>(path: string, body: object) => request<T>(path, {
  method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify(body),
});
export function listChildTasks(parent: string, after: string | null, signal: AbortSignal) {
  return request<ChildTaskPage>(`${base(parent)}?limit=50${after ? `&after=${encodeURIComponent(after)}` : ""}`, { signal });
}
export async function listChildProposals(parent: string, signal: AbortSignal): Promise<ChildProposalPage> {
  try { return await request<ChildProposalPage>(`${base(parent)}/proposals`, { signal }); }
  catch (error) {
    // Older Web servers have no proposal route; existing task controls remain usable.
    if (error instanceof ChildSessionFailure && [404, 501].includes(error.status)) return { proposals: [] };
    throw error;
  }
}
const proposalPath = (parent: string, proposal: string) => `${base(parent)}/proposals/${encodeURIComponent(proposal)}`;
export const startChildProposal = (parent: string, proposal: string, requestId: string) => post(`${proposalPath(parent, proposal)}/start`, { request_id: requestId });
export const dismissChildProposal = (parent: string, proposal: string, requestId: string) => post<{ proposal_id: string; status: "dismissed" }>(`${proposalPath(parent, proposal)}/dismiss`, { request_id: requestId });
export async function getChildCapabilities(parent: string, signal: AbortSignal): Promise<ChildCapabilities> {
  try { return await request<ChildCapabilities>(`${base(parent)}/capabilities`, { signal }); }
  catch (error) {
    if (error instanceof ChildSessionFailure && [404, 501].includes(error.status)) return { capabilities: [] };
    throw error;
  }
}
const taskOptionsBody = (options: ChildTaskOptions) => ({
  ...(options.subscription !== undefined ? { subscription: options.subscription } : {}),
  ...(options.dependsOn !== undefined ? { depends_on: options.dependsOn } : {}),
});
export const startChildTask = (parent: string, options: StartChildOptions, requestId: string) => post(base(parent), {
  title: options.title, prompt: options.prompt, request_id: requestId,
  context: options.context ?? "fresh",
  through_turn_id: options.throughTurnId ?? null,
  workspace_mode: options.workspaceMode ?? "shared_read_only",
  worktree_base: options.worktreeBase ?? "head",
  model: options.model ?? null, ...taskOptionsBody(options),
});
export const continueChildTask = (parent: string, child: string, prompt: string, requestId: string, options: ChildTaskOptions = {}) => post(`${base(parent)}/${encodeURIComponent(child)}/continue`, { prompt, request_id: requestId, ...taskOptionsBody(options) });
export const retryChildTask = (parent: string, task: ChildTask, requestId: string) => post(`${base(parent)}/${encodeURIComponent(task.assignment_id)}/retry`, { expected_result_revision: task.result?.revision, request_id: requestId });
export const stopChildTask = (parent: string, assignment: string, requestId: string) => post(`${base(parent)}/${encodeURIComponent(assignment)}/stop`, { request_id: requestId });
const linkPath = (parent: string, child: string) => `${base(parent)}/links/${encodeURIComponent(child)}`;
export const getChildLink = (parent: string, child: string, signal: AbortSignal) => request<ChildLink>(linkPath(parent, child), { signal });
export const changeChildControl = (parent: string, child: string, kind: "takeover" | "detach", revision: number, requestId: string) => post<ChildLink>(`${linkPath(parent, child)}/${kind}`, { request_id: requestId, expected_control_revision: revision });
export const setChildSubscription = (parent: string, task: ChildTask, subscription: ChildSubscription, requestId: string) => post(`${base(parent)}/${encodeURIComponent(task.assignment_id)}/subscription`, { request_id: requestId, expected_subscription_revision: task.subscription_revision ?? 1, subscription });
