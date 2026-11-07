import type { Messages } from "./i18n";

/** The server projects failure_detail; raw message strings are not UI text. */
export type RuntimeFailure = {
  message?: string;
  session_id?: string;
  turn_id?: string;
  task_id?: string | null;
  reason?: string | null;
  failure_domain?: string | null;
  failure_detail?: string | null;
};

const failureReasonMessageKeys: Record<string, keyof Messages> = {
  context_overflow: "turnFailureContextOverflow",
  rate_limited: "turnFailureRateLimited",
  auth: "turnFailureAuth",
  quota: "turnFailureQuota",
  stream_timeout: "turnFailureStreamTimeout",
  timeout: "turnFailureTimeout",
  network: "turnFailureNetwork",
  provider_unavailable: "turnFailureProviderUnavailable",
  execution_owner_conflict: "turnFailureOwnerConflict",
  unclassified: "turnFailureUnclassified",
  runtime_restarted: "turnFailureRuntimeRestarted",
};
const failureDomainMessageKeys: Record<string, keyof Messages> = {
  provider: "failureProvider", policy: "failurePolicy", tool: "failureTool",
  harness: "failureHarness", internal: "failureInternal", unknown: "failureInternal",
};

export function runtimeFailureMessage(event: RuntimeFailure, t: Messages): string {
  if (event.message === "turn.cancelled") return t.turnFailureCancelled;
  const detail = event.failure_detail?.trim();
  const reasonKey = event.reason ? failureReasonMessageKeys[event.reason] : undefined;
  const fallback = event.message === "turn.interrupted"
    ? t.turnFailureInterrupted : reasonKey ? t[reasonKey] : t.turnFailureUnknown;
  const domainKey = event.failure_domain ? failureDomainMessageKeys[event.failure_domain] : undefined;
  const domain = domainKey ? ` · ${t.failureDomain}: ${t[domainKey]}` : "";
  return `${t.requestFailed}: ${detail || fallback}${domain}`;
}

export function runtimeFailureReference(event: RuntimeFailure, sessionId: string): { sessionId: string; taskId: string } | null {
  if (event.session_id !== sessionId || !event.task_id || !["turn.failed", "turn.interrupted"].includes(event.message ?? "")) return null;
  return { sessionId, taskId: event.task_id };
}
