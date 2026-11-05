import type { Language } from "./i18n";
import { childSessionsText } from "./childSessionsMessages";
import { childTaskControlled, childTaskSubscription, ChildSessionFailure, type ChildSubscription, type ChildTask } from "./childSessionsApi";

export type TaskOptionsDraft = { subscription: ChildSubscription; dependsOn: string[] };
export const emptyTaskOptions = (): TaskOptionsDraft => ({ subscription: "auto_wake", dependsOn: [] });
export const invalidDependencies = (selected: string[], tasks: ChildTask[]) => selected.some((id) => !tasks.some((task) => task.assignment_id === id && childTaskControlled(task)));
export function childFailureText(error: unknown, language: Language) {
  const t = childSessionsText(language);
  if (!(error instanceof ChildSessionFailure)) return t.error;
  if (error.status === 403) return t.forbidden;
  if (error.status === 409) return t.conflict;
  if ([404, 501].includes(error.status)) return t.controlUnavailable;
  return `${t.error} ${error.message}`;
}
export function childNotificationLabel(task: ChildTask, language: Language) {
  const t = childSessionsText(language);
  if (task.result?.consumed_at != null) return t.consumed;
  if (!childTaskControlled(task)) return t.notificationStopped;
  if (task.subscription === undefined && task.notify === undefined) return t.pending;
  return { none: t.notificationOff, inbox: t.notificationInbox, auto_wake: t.notificationAuto }[childTaskSubscription(task)];
}
