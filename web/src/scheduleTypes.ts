export type ScheduleSpec = { kind: "interval"; minutes: number }
  | { kind: "dailyAt" | "weekdaysAt"; hour: number; minute: number }
  | { kind: "weeklyAt"; weekday: number; hour: number; minute: number }
  | { kind: "onceAt"; at: number };
export type ScheduleDraft = {
  name: string; prompt: string; workspace: string; schedule: ScheduleSpec; timezone: string;
  goal: string | null; approval_mode: string | null; profile: string | null; model: string | null;
  max_turns: number | null; timeout_seconds: number | null; token_budget: number | null;
  missed_policy: "coalesce" | "skip"; failure_limit: number; retention_days: number;
};
export type ScheduledTask = ScheduleDraft & { id: string; enabled: boolean; revision: number; created_at: number;
  updated_at: number; archived_at: number | null; completed_at: number | null; migration_reason: string | null; consecutive_failures: number };
export const runStatuses = ["preparing", "queued", "running", "waiting_approval", "waiting_answer", "completed", "partial", "failed", "cancelled", "skipped", "unknown"] as const;
export type ScheduleRunStatus = (typeof runStatuses)[number];
export type ScheduleRun = {
  id: string; task_id: string; sequence: number; trigger: "scheduled" | "manual" | "retry" | "imported";
  scheduled_at: number; queued_at: number | null; started_at: number | null; finished_at: number | null;
  session_id: string | null; turn_id: string | null; retry_of: string | null; status: ScheduleRunStatus;
  reason: string | null; result: string | null; verification: "passed" | "failed" | null;
  usage: { input_tokens: number; output_tokens: number; total_tokens: number; model_calls: number | null } | null;
  snapshot: ScheduledTask;
};
export type ScheduleView = { task: ScheduledTask; next_fire_at: number | null; last_run: ScheduleRun | null; active_run?: ScheduleRun | null; deleted?: boolean; deleted_at?: number };
export type ScheduleList = { tasks: ScheduleView[]; writable: boolean };
export type ScheduleHistory = { runs: ScheduleRun[]; next_cursor: string | null };
export type ScheduleWorkspace = { id: string; path: string; name: string };
export type ScheduleModels = { default_provider: string; root_model: string; providers: { id: string; model: string }[];
  profiles: { id: string; effective_provider: string; effective_model: string }[] };
const ACTIVE_STATUSES: ReadonlySet<ScheduleRunStatus> = new Set(["preparing", "queued", "running", "waiting_approval", "waiting_answer"]);
export function runIsActive(status: ScheduleRunStatus) { return ACTIVE_STATUSES.has(status); }
export function runCanRetry(status: ScheduleRunStatus) { return ["failed", "partial", "cancelled", "unknown"].includes(status); }
export class ScheduleRequestError extends Error {
  readonly status: number;
  constructor(status: number, message: string) { super(message); this.status = status; }
}
export async function scheduleJson<T>(path: string, init?: RequestInit): Promise<T> {
  const response = await fetch(path, init);
  if (!response.ok && !response.headers.get("content-type")?.includes("application/json")) {
    throw new ScheduleRequestError(response.status, (await response.text()).slice(0, 500) || response.statusText);
  }
  const value: unknown = await response.json();
  if (!response.ok) {
    const message = typeof value === "object" && value !== null && "error" in value && typeof value.error === "string" ? value.error : response.statusText;
    throw new ScheduleRequestError(response.status, message);
  }
  return value as T;
}
export function scheduleWrite<T>(path: string, value: unknown, method = "POST") {
  return scheduleJson<T>(path, { method, headers: { "content-type": "application/json" }, body: JSON.stringify(value) });
}
export function newSchedule(workspace: string, timezone: string): ScheduleDraft {
  return { name: "", prompt: "", workspace, timezone, schedule: { kind: "interval", minutes: 30 }, goal: null,
    approval_mode: null, profile: null, model: null, max_turns: null, timeout_seconds: null, token_budget: null,
    missed_policy: "coalesce", failure_limit: 3, retention_days: 90 };
}
export function taskDraft(task: ScheduledTask): ScheduleDraft {
  const { name, prompt, workspace, timezone, schedule, goal, approval_mode, profile, model, max_turns, timeout_seconds, token_budget, missed_policy, failure_limit, retention_days } = task;
  return { name, prompt, workspace, timezone, schedule, goal, approval_mode, profile, model, max_turns, timeout_seconds, token_budget, missed_policy, failure_limit, retention_days };
}
