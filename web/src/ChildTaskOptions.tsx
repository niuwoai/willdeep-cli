import { useRef, useState } from "react";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import type { Language } from "./i18n";
import { childSessionsText } from "./childSessionsMessages";
import { childTaskControlled, childTaskSubscription, ChildSessionFailure, setChildSubscription, type ChildSubscription, type ChildTask } from "./childSessionsApi";
import { CHILD_DEPENDENCY_LIMIT, childTasksKey, updateChildTask, type ChildFeatures } from "./childSessionsQueries";
import { childFailureText, invalidDependencies, type TaskOptionsDraft } from "./childSessionsPresentation";

export function ChildTaskOptionsFields({ value, onChange, tasks, features, disabled, language }: {
  value: TaskOptionsDraft; onChange: (value: TaskOptionsDraft) => void; tasks: ChildTask[];
  features: ChildFeatures; disabled: boolean; language: Language;
}) {
  const t = childSessionsText(language);
  const candidates = tasks.filter((task) => childTaskControlled(task) || value.dependsOn.includes(task.assignment_id));
  const unavailable = value.dependsOn.filter((id) => !tasks.some((task) => task.assignment_id === id));
  return <div className="child-task-options">
    {features.subscriptions ? <><label>{t.subscriptionLabel}<select value={value.subscription} disabled={disabled} onChange={(event) => onChange({ ...value, subscription: event.target.value as ChildSubscription })}>
      {Object.entries(t.subscriptions).map(([key, label]) => <option key={key} value={key}>{label}</option>)}
    </select></label><p className="child-session-hint">{t.subscriptionHint}</p></> : <p className="child-session-hint">{t.subscriptionUnsupported}</p>}
    {features.dependencies ? <fieldset className="child-dependencies" disabled={disabled}>
      <legend>{t.dependenciesLabel}</legend><p className="child-session-hint">{t.dependenciesHint.replace("{limit}", String(CHILD_DEPENDENCY_LIMIT))}</p>
      {!candidates.length && !unavailable.length && <p className="child-session-hint">{t.dependenciesEmpty}</p>}
      {candidates.map((task) => {
        const checked = value.dependsOn.includes(task.assignment_id);
        return <label className="child-dependency-choice" key={task.assignment_id}>
          <input type="checkbox" checked={checked} disabled={!checked && (!childTaskControlled(task) || value.dependsOn.length >= CHILD_DEPENDENCY_LIMIT)} onChange={(event) => onChange({ ...value, dependsOn: event.target.checked ? [...value.dependsOn, task.assignment_id] : value.dependsOn.filter((id) => id !== task.assignment_id) })} />
          <span>{task.title} <code>{task.assignment_id}</code><small>{task.status === "preparing" && task.waiting_on_dependencies ? t.waitingDependencies : t.statuses[task.status]}{!childTaskControlled(task) ? ` · ${t.controlStates[task.control_state!]}` : ""}</small></span>
        </label>;
      })}
      {unavailable.map((id) => <label className="child-dependency-choice" key={id}><input type="checkbox" checked onChange={() => onChange({ ...value, dependsOn: value.dependsOn.filter((other) => other !== id) })} /><span>{t.dependencyUnavailable} <code>{id}</code></span></label>)}
      {invalidDependencies(value.dependsOn, tasks) && <p className="child-session-error" role="alert">{t.dependenciesStale}</p>}
    </fieldset> : <p className="child-session-hint">{t.dependenciesUnsupported}</p>}
  </div>;
}

export function ChildTaskSubscription({ task, language, disabled }: { task: ChildTask; language: Language; disabled: boolean }) {
  const t = childSessionsText(language);
  const cache = useQueryClient();
  const [selection, setSelection] = useState<ChildSubscription | null>(null);
  const identities = useRef(new Map<string, string>());
  const subscription = selection ?? childTaskSubscription(task);
  const consumed = task.result?.consumed_at != null;
  const mutation = useMutation({
    mutationFn: ({ value, snapshot }: { value: ChildSubscription; snapshot: ChildTask }) => {
      const key = `${snapshot.subscription_revision ?? 1}:${value}`;
      let id = identities.current.get(key);
      if (!id) { id = crypto.randomUUID(); identities.current.set(key, id); }
      return setChildSubscription(task.parent_session_id, snapshot, value, id);
    },
    onSuccess: async (updated, action) => {
      identities.current.delete(`${action.snapshot.subscription_revision ?? 1}:${action.value}`);
      updateChildTask(cache, task.parent_session_id, updated); setSelection(null);
      await cache.invalidateQueries({ queryKey: childTasksKey(task.parent_session_id) });
    },
    onError: async (error) => {
      if (error instanceof ChildSessionFailure && error.status === 409) await cache.invalidateQueries({ queryKey: childTasksKey(task.parent_session_id) });
    },
  });
  const locked = disabled || mutation.isPending || consumed || !childTaskControlled(task);
  return <div className="child-subscription-editor">
    <label>{t.subscriptionLabel}<select value={subscription} disabled={locked} onChange={(event) => { setSelection(event.target.value as ChildSubscription); mutation.reset(); }}>
      {Object.entries(t.subscriptions).map(([key, label]) => <option key={key} value={key}>{label}</option>)}
    </select></label>
    <button type="button" disabled={locked || subscription === childTaskSubscription(task)} onClick={() => mutation.mutate({ value: subscription, snapshot: task })}>{mutation.isPending ? t.working : t.saveSubscription}</button>
    <p className="child-session-hint">{consumed ? t.subscriptionConsumed : t.subscriptionHint}</p>
    {mutation.error && <p role="alert" className="child-session-error">{childFailureText(mutation.error, language)}</p>}
    {mutation.isSuccess && <p role="status">{t.subscriptionSaved}</p>}
  </div>;
}

