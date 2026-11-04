import { useMutation, useQuery, useQueryClient, type QueryClient, type QueryKey } from "@tanstack/react-query";
import type { AgentUsageSummary } from "./RuntimeSidebar";

export type JsonValue = null | boolean | number | string | JsonValue[] | { [key: string]: JsonValue };
export type WorkflowCondition =
  | { op: "eq" | "ne" | "lt" | "le" | "gt" | "ge"; left: JsonValue; right: JsonValue }
  | { op: "and" | "or"; conditions: WorkflowCondition[] }
  | { op: "not"; condition: WorkflowCondition };
export type WorkflowScope = { nodes: WorkflowNode[]; output: JsonValue };
export type WorkflowNode = { id: string; depends_on: string[]; phase: string | null } & (
  | { kind: "agent"; profile: string; prompt: string; input: JsonValue; output_schema: JsonValue;
      worker_tier: "standard" | "advanced" | "expert" | null; max_turns: number | null;
      token_budget: number | null; timeout_seconds: number | null; write_targets: string[]; task_packet: JsonValue;
      escalation?: JsonValue; target_command?: string | null }
  | { kind: "switch"; condition: WorkflowCondition; then_scope: WorkflowScope; else_scope: WorkflowScope }
  | { kind: "foreach"; items: JsonValue; body: WorkflowScope }
  | { kind: "repeat"; initial: JsonValue; body: WorkflowScope; until: WorkflowCondition; carry: JsonValue; max_iterations: number }
);
export type WorkflowSpec = { version: number; id: string; name: string; input_schema: JsonValue;
  phases: { id: string; name: string }[]; nodes: WorkflowNode[]; output: JsonValue };
export type WorkflowLimits = {
  max_definition_bytes: number; max_run_bytes: number; max_nodes: number; max_depth: number;
  max_instances: number; max_attempts: number; max_foreach_items: number; max_iterations: number;
  max_concurrent: number; max_value_bytes: number; max_report_bytes: number; max_name_bytes: number;
};
export type WorkflowAgentRun = {
  id: string; request_id: string; agent_id: string; parent_session_id: string;
  workflow_owner?: { workflow_run_id: string; node_instance_id: string };
  sequence: number; reason: "spawn" | "continue" | "retry" | "recover"; status: string; label: string;
  model: string | null; queued_at_ms: number; started_at_ms: number | null; finished_at_ms: number | null;
  max_turns: number | null; token_budget: number | null; timeout_seconds: number | null;
  current_turn: number; current_tool: string | null; usage: AgentUsageSummary; report: string | null; delivered: boolean;
};
export type WorkflowNodeStatus = "pending" | "dispatch_pending" | "dispatched" | "running" | "stopping" |
  "completed" | "failed" | "blocked" | "interrupted" | "cancelled";
export type WorkflowNodeInstance = {
  id: string; node_id: string; path: string; attempt_id: string; status: WorkflowNodeStatus;
  request_id: string | null; agent_run: WorkflowAgentRun | null; output: JsonValue; report: string | null;
  error: string | null; usage: AgentUsageSummary; created_at_ms: number; started_at_ms: number | null;
  finished_at_ms: number | null; reused_from: { run_id: string; node_instance_id: string } | null;
};
export type WorkflowRun = {
  id: string; revision?: number; parent_session_id: string; definition_id: string; spec: WorkflowSpec; input: JsonValue;
  limits: WorkflowLimits; status: "running" | "stopping" | "completed" | "failed" | "cancelled" | "interrupted";
  nodes: WorkflowNodeInstance[]; output: JsonValue; report: string | null; error: string | null;
  created_at_ms: number; finished_at_ms: number | null; source_run_id: string | null; acknowledged: boolean;
};
export type WorkflowList = { definitions: WorkflowSpec[]; runs: WorkflowRun[] };
export type WorkflowAction = "stop" | "retry" | "resume";
export class WorkflowApiError extends Error {
  constructor(readonly status: number, message: string, readonly code?: string) { super(message); }
}

export async function workflowCall<T>(operation: string, parentSessionId: string, params: Record<string, unknown> = {}, signal?: AbortSignal): Promise<T> {
  const response = await fetch(`/api/workflows/${operation}`, {
    method: "POST", headers: { "content-type": "application/json" },
    body: JSON.stringify({ ...params, parent_session_id: parentSessionId }), signal,
  });
  const value: unknown = await response.json().catch(() => null);
  if (!response.ok) {
    const error = isRecord(value) ? value.error : null;
    const detail = typeof error === "string" ? error : isRecord(error) && typeof error.message === "string" ? error.message : response.statusText;
    const code = isRecord(error) && typeof error.code === "string" ? error.code : undefined;
    throw new WorkflowApiError(response.status, detail || String(response.status), code);
  }
  return value as T;
}
export const workflowKeys = {
  list: (parent: string) => ["workflows", parent, "list"] as const,
  run: (parent: string, id: string) => ["workflows", parent, "run", id] as const,
  history: (parent: string, definition: string) => ["workflows", parent, "history", definition] as const,
};
export const workflowActive = (run: WorkflowRun) => run.status === "running" || run.status === "stopping";
export const WORKFLOW_POLL_MS = 3000;

// A retry/resume has a new ID. A single run never returns from terminal to active,
// or from stopping to running, even when an older control receipt arrives late.
function latestWorkflow(current: WorkflowRun | undefined, incoming: WorkflowRun): WorkflowRun {
  if (!current) return incoming;
  if ((incoming.revision ?? 0) < (current.revision ?? 0)) return current;
  if ((!workflowActive(current) && workflowActive(incoming)) || (current.status === "stopping" && incoming.status === "running")) return current;
  return current.acknowledged && !incoming.acknowledged ? { ...incoming, acknowledged: true } : incoming;
}
function sameKey(left: QueryKey, right: QueryKey | undefined) {
  return right !== undefined && left.length === right.length && left.every((part, index) => part === right[index]);
}
// Queries, control receipts and SSE all publish to the same run/list/history caches.
// The source query commits its own result; cancel only other reads of this run so
// unrelated workers can keep polling. Cancellation is synchronous before the writes.
export function cacheWorkflow(client: QueryClient, incoming: WorkflowRun, source?: QueryKey): WorkflowRun {
  const parent = incoming.parent_session_id;
  const runKey = workflowKeys.run(parent, incoming.id);
  const workflow = latestWorkflow(client.getQueryData<WorkflowRun>(runKey), incoming);
  if (workflow !== incoming && workflow === client.getQueryData<WorkflowRun>(runKey)) return workflow;
  const listKey = workflowKeys.list(parent);
  const historyKey = workflowKeys.history(parent, workflow.definition_id);
  // An event/control receipt supersedes outstanding reads. Reads must not cancel
  // one another while they populate details and history from the same snapshot.
  if (!source) void client.cancelQueries({ predicate: ({ queryKey }) =>
    sameKey(queryKey, runKey) || sameKey(queryKey, listKey) || sameKey(queryKey, historyKey) });
  if (!sameKey(runKey, source)) client.setQueryData(runKey, workflow);
  const upsert = (runs: WorkflowRun[]) => [workflow, ...runs.filter((run) => run.id !== workflow.id)]
    .sort((a, b) => b.created_at_ms - a.created_at_ms);
  if (!sameKey(listKey, source)) client.setQueryData<WorkflowList>(listKey, (old) => ({ definitions: old?.definitions ?? [], runs: upsert(old?.runs ?? []) }));
  if (!sameKey(historyKey, source)) client.setQueryData<{ runs: WorkflowRun[] }>(historyKey, (old) => old ? { runs: upsert(old.runs) } : undefined);
  return workflow;
}
function isRecord(value: unknown): value is Record<string, unknown> { return typeof value === "object" && value !== null; }
export function workflowFromEvent(event: unknown): WorkflowRun | null {
  if (!isRecord(event) || (event.type !== "workflow_updated" && event.kind !== "workflow_updated")) return null;
  const run = event.workflow ?? (isRecord(event.data) ? event.data.workflow : null);
  if (!isRecord(run) || typeof run.id !== "string" || typeof run.parent_session_id !== "string" ||
    typeof run.definition_id !== "string" || !isRecord(run.spec) || !Array.isArray(run.spec.nodes) ||
    !Array.isArray(run.nodes) || typeof run.status !== "string") return null;
  return run as WorkflowRun;
}
export function useWorkflowList(parent: string) {
  const client = useQueryClient();
  const queryKey = workflowKeys.list(parent);
  return useQuery({ queryKey, enabled: Boolean(parent),
    queryFn: async ({ signal }) => {
      const result = await workflowCall<WorkflowList>("list", parent, {}, signal);
      // A fetch implementation may ignore abort; its query function must not publish.
      signal.throwIfAborted();
      return { ...result, runs: result.runs.filter((run) => run.parent_session_id === parent)
        .map((run) => cacheWorkflow(client, run, queryKey)) };
    },
    refetchInterval: (query) => query.state.data?.runs.some(workflowActive) ? WORKFLOW_POLL_MS : false,
    refetchOnReconnect: "always", refetchOnWindowFocus: true, retry: false });
}
export function useWorkflowRun(snapshot: WorkflowRun) {
  const client = useQueryClient();
  const queryKey = workflowKeys.run(snapshot.parent_session_id, snapshot.id);
  return useQuery({ queryKey,
    queryFn: async ({ signal }) => {
      const result = await workflowCall<{ workflow: WorkflowRun }>("get", snapshot.parent_session_id, { run_id: snapshot.id }, signal);
      signal.throwIfAborted();
      return cacheWorkflow(client, result.workflow, queryKey);
    },
    initialData: snapshot, initialDataUpdatedAt: 0,
    refetchInterval: (query) => query.state.data && workflowActive(query.state.data) ? WORKFLOW_POLL_MS : false,
    refetchOnReconnect: "always", refetchOnWindowFocus: true, retry: false });
}
export function useWorkflowHistory(parent: string, definition: string, enabled: boolean) {
  const client = useQueryClient();
  const queryKey = workflowKeys.history(parent, definition);
  return useQuery({ queryKey, enabled: enabled && Boolean(parent),
    queryFn: async ({ signal }) => {
      const result = await workflowCall<{ runs: WorkflowRun[] }>("history", parent, { definition_id: definition }, signal);
      signal.throwIfAborted();
      return { runs: result.runs.filter((run) => run.parent_session_id === parent && run.definition_id === definition)
        .map((run) => cacheWorkflow(client, run, queryKey)) };
    },
    refetchOnReconnect: "always", retry: false });
}
export function useWorkflowControl(parent: string) {
  const client = useQueryClient();
  return useMutation({ mutationFn: async ({ action, runId }: { action: WorkflowAction; runId: string }) => {
    const result = await workflowCall<{ workflow: WorkflowRun }>(action, parent, { run_id: runId, request_id: crypto.randomUUID() });
    return { workflow: cacheWorkflow(client, result.workflow) };
  },
  onSuccess: ({ workflow }) => {
    void client.invalidateQueries({ queryKey: workflowKeys.list(parent) });
    void client.invalidateQueries({ queryKey: workflowKeys.history(parent, workflow.definition_id) });
  }, retry: false });
}
export function runWorkflow(parent: string, definitionId: string, input: JsonValue, requestId = crypto.randomUUID()) {
  return workflowCall<{ workflow: WorkflowRun }>("run", parent, { definition_id: definitionId, input, request_id: requestId });
}
export function loadWorkflowSchema(parent: string, signal?: AbortSignal) {
  return workflowCall<{ schema: JsonValue; example: WorkflowSpec }>("schema", parent, {}, signal);
}
