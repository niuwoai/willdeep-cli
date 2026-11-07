import { useRef, useState } from "react";
import { useInfiniteQuery, useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { ArrowLeft, ArrowUpRight, GitBranch, RefreshCw } from "lucide-react";
import type { Language } from "./i18n";
import { childSessionsText } from "./childSessionsMessages";
import { childProposalText } from "./childProposalMessages";
import { useChildProposals } from "./childProposalQueries";
import { ChildSessionProposals } from "./ChildSessionProposals";
import { ChildSessionFailure, childTaskControlled, childTaskRetryable, childTaskTerminal, continueChildTask, getChildLink, listChildTasks, retryChildTask, stopChildTask, type ChildControlState, type ChildLink, type ChildTask, type ChildTaskOptions } from "./childSessionsApi";
import { ChildSessionControl } from "./ChildSessionControl";
import { ChildTaskOptionsFields, ChildTaskSubscription } from "./ChildTaskOptions";
import { childFailureText, childNotificationLabel, emptyTaskOptions, invalidDependencies, type TaskOptionsDraft } from "./childSessionsPresentation";
import { childLinkKey, childTasksKey, useChildCapabilities } from "./childSessionsQueries";
import "./childSessions.css";

const BRIEF_LIMIT = 16_000;
type Action = { kind: "continue"; child: string; prompt: string; options: ChildTaskOptions } | { kind: "retry"; task: ChildTask } | { kind: "stop"; task: ChildTask };

export function ChildSessions({ sessionId, language, onOpenSession, onRelationshipChanged }: {
  sessionId: string; language: Language; onOpenSession: (id: string) => void; onRelationshipChanged?: (link: ChildLink) => void;
}) {
  const t = childSessionsText(language);
  const p = childProposalText(language);
  const cache = useQueryClient();
  const queryKey = childTasksKey(sessionId);
  const capabilities = useChildCapabilities(sessionId);
  const proposals = useChildProposals(sessionId);
  const tasks = useInfiniteQuery({
    queryKey, initialPageParam: null as string | null,
    queryFn: ({ pageParam, signal }) => listChildTasks(sessionId, pageParam, signal),
    getNextPageParam: (page) => page.next_cursor ?? undefined,
    refetchInterval: 4000, retry: false,
  });
  const [continuation, setContinuation] = useState<({ child: string; prompt: string } & TaskOptionsDraft) | null>(null);
  const identities = useRef(new Map<string, string>());
  const mutation = useMutation({
    mutationFn: (action: Action) => {
      const fingerprint = JSON.stringify([sessionId, action]);
      let id = identities.current.get(fingerprint);
      if (!id) { id = crypto.randomUUID(); identities.current.set(fingerprint, id); }
      switch (action.kind) {
        case "continue": return continueChildTask(sessionId, action.child, action.prompt, id, action.options);
        case "retry": return retryChildTask(sessionId, action.task, id);
        case "stop": return stopChildTask(sessionId, action.task.assignment_id, id);
      }
    },
    onSuccess: async (_, action) => {
      if (action.kind === "continue") setContinuation(null);
      identities.current.delete(JSON.stringify([sessionId, action]));
      await cache.invalidateQueries({ queryKey });
    },
    onError: async (error) => {
      if (error instanceof ChildSessionFailure && error.status === 409) await cache.invalidateQueries({ queryKey });
    },
  });
  const failure = mutation.error ?? tasks.error ?? proposals.error;
  const errorMessage = failure instanceof ChildSessionFailure && [404, 501].includes(failure.status) ? t.unsupported : childFailureText(failure, language);
  const allTasks = tasks.data?.pages.flatMap((page) => page.tasks) ?? [];
  const uniqueTasks = [...new Map(allTasks.map((task) => [task.assignment_id, task])).values()].sort((a, b) => b.created_at - a.created_at);
  const pendingProposals = (proposals.data?.proposals ?? []).filter((proposal) => !proposal.dismissed && proposal.params.parent_session_id === sessionId);
  const pending = mutation.isPending;
  // 404/501 resolve as confirmed legacy support; failed capability reads must block new work.
  const capabilitiesReady = capabilities.isSuccess;
  const validPrompt = (prompt: string) => !!prompt.trim() && Array.from(prompt.trim()).length <= BRIEF_LIMIT;
  const options = (value: TaskOptionsDraft): ChildTaskOptions => ({
    ...(capabilities.subscriptions ? { subscription: value.subscription } : {}),
    ...(capabilities.dependencies ? { dependsOn: value.dependsOn } : {}),
  });
  const dependenciesValid = (value: TaskOptionsDraft) => !capabilities.dependencies || !invalidDependencies(value.dependsOn, uniqueTasks);
  if (!uniqueTasks.length && !pendingProposals.length && !failure && !capabilities.error) return null;
  const refreshing = tasks.isFetching || proposals.isFetching || capabilities.isFetching;
  const refresh = () => { void tasks.refetch(); void proposals.refetch(); void capabilities.refetch(); };
  return <section className="child-sessions" aria-label={t.title}>
    <header className="child-sessions-heading"><h2><GitBranch size={16} />{t.title}</h2><button type="button" disabled={refreshing} aria-label={t.refresh} title={t.refresh} onClick={refresh}><RefreshCw size={14} /></button></header>
    {failure && <p className="child-session-error" role="alert">{proposals.error === failure && !(failure instanceof ChildSessionFailure) ? p.error : errorMessage}</p>}
    {capabilities.error && !failure && <p className="child-session-error" role="alert">{childFailureText(capabilities.error, language)}</p>}
    {(tasks.error || proposals.error || capabilities.error) && <button type="button" disabled={refreshing} onClick={refresh}>{p.retry}</button>}
    <ChildSessionProposals key={sessionId} sessionId={sessionId} proposals={pendingProposals} tasks={uniqueTasks} language={language} />
    <ol className="child-task-list">{uniqueTasks.map((task) => {
      const terminal = childTaskTerminal(task.status);
      const controlled = childTaskControlled(task);
      const result = task.result;
      const childBusy = uniqueTasks.some((other) => other.child_session_id === task.child_session_id && !childTaskTerminal(other.status));
      const latest = uniqueTasks.find((other) => other.child_session_id === task.child_session_id)?.assignment_id === task.assignment_id;
      return <li className={`child-task ${terminal ? "terminal" : "active"}`} key={task.assignment_id}>
        <header><strong>{task.title}</strong><span className={`child-task-state ${task.status}`}>{task.status === "preparing" && task.waiting_on_dependencies ? t.waitingDependencies : t.statuses[task.status] ?? t.unknown}</span></header>
        <p className="child-session-scope">{task.workspace_mode === "worktree" ? t.worktree : t.readOnly}{task.context === "fork" ? ` · ${t.fork}` : ""}{task.model ? ` · ${task.model}` : ""}</p>
        {task.control_state && <p className="child-session-scope">{t.controlStates[task.control_state]}</p>}
        {!controlled && <p className="child-session-hint">{t.parentControlDisabled}</p>}
        <details className="child-task-brief"><summary>{t.briefToggle}</summary><p>{task.prompt}</p></details>
        {!!task.depends_on?.length && <div className="child-dependency-history"><span>{t.dependencyHistory}</span><ul>{task.depends_on.map((id) => <li key={id}>{uniqueTasks.find((other) => other.assignment_id === id)?.title ?? t.dependencyUnavailable} <code>{id}</code></li>)}</ul></div>}
        {task.error && <p className="child-session-error">{task.error}</p>}
        {result && <details className="child-task-result"><summary>{t.result}<span>{childNotificationLabel(task, language)}{result.superseded_by ? ` · ${t.superseded}` : ""}</span></summary>
          <p>{result.report || result.summary || t.noReport}</p>{!result.report_complete && <p className="child-session-hint">{t.incomplete}</p>}
          {result.worktree && <p className="child-session-scope">{t.worktreeBranch}: {result.worktree.branch} · {t.worktreeBase}: {result.worktree.base_commit}{result.worktree.base === "snapshot" && result.worktree.untracked_files > 0 ? ` · ${result.worktree.untracked_files} ${t.worktreeUntracked}` : ""}</p>}
          {result.worktree?.base === "head" && result.worktree.parent_dirty && <p className="child-session-hint">{t.worktreeDirty}</p>}
        </details>}
        {terminal && !result && <p className="child-session-hint">{t.noReport}</p>}
        {capabilities.subscriptions && <ChildTaskSubscription task={task} language={language} disabled={pending} />}
        <div className="child-task-actions"><button type="button" onClick={() => onOpenSession(task.child_session_id)}>{t.open}<ArrowUpRight size={13} /></button>
          {!terminal && <button type="button" disabled={pending || !controlled} onClick={() => mutation.mutate({ kind: "stop", task })}>{t.stop}</button>}
          {childTaskRetryable(task) && <button type="button" disabled={pending || childBusy || !controlled} onClick={() => mutation.mutate({ kind: "retry", task })}>{t.retry}</button>}
          {terminal && !result?.superseded_by && <button type="button" disabled={pending || childBusy || !controlled} onClick={() => setContinuation(continuation?.child === task.child_session_id ? continuation : { child: task.child_session_id, prompt: "", ...emptyTaskOptions() })}>{t.continue}</button>}
        </div>
        {capabilities.control && latest && <ChildSessionControl parentId={sessionId} childId={task.child_session_id} state={task.control_state} language={language} onChanged={onRelationshipChanged} />}
        {continuation?.child === task.child_session_id && terminal && !result?.superseded_by && latest && <form className="child-continue" onSubmit={(event) => {
          event.preventDefault(); if (!pending && controlled && capabilitiesReady && validPrompt(continuation.prompt) && dependenciesValid(continuation)) mutation.mutate({ kind: "continue", child: continuation.child, prompt: continuation.prompt.trim(), options: options(continuation) });
        }}>
          <label>{t.continueLabel}<textarea value={continuation.prompt} disabled={pending || !controlled} onChange={(event) => setContinuation({ ...continuation, prompt: event.target.value })} /></label>
          {capabilities.data && <ChildTaskOptionsFields value={continuation} onChange={(value) => setContinuation({ ...continuation, ...value })} tasks={uniqueTasks} features={capabilities} disabled={pending || !controlled || !capabilitiesReady} language={language} />}
          {tasks.hasNextPage && capabilities.dependencies && <button type="button" disabled={tasks.isFetchingNextPage} onClick={() => void tasks.fetchNextPage()}>{t.more}</button>}
          <footer><button type="button" disabled={pending} onClick={() => setContinuation(null)}>{t.cancel}</button><button type="submit" disabled={pending || !controlled || !capabilitiesReady || !validPrompt(continuation.prompt) || !dependenciesValid(continuation)}>{t.submit}</button></footer>
        </form>}
      </li>;
    })}</ol>
    {tasks.hasNextPage && <button className="child-more" type="button" disabled={tasks.isFetchingNextPage} onClick={() => void tasks.fetchNextPage()}>{t.more}</button>}
  </section>;
}

type OriginProps = {
  parentId: string; parentTitle?: string; childId?: string; readOnly?: boolean; detached?: boolean;
  language: Language; onOpenSession: (id: string) => void; onRelationshipChanged?: (link: ChildLink) => void;
};
export function ChildSessionOrigin(props: OriginProps) {
  if (props.childId) return <ChildSessionOriginWithControl {...props} childId={props.childId} />;
  return <OriginContent {...props} state={props.detached ? "detached" : "delegated"} />;
}
function ChildSessionOriginWithControl(props: OriginProps & { childId: string }) {
  const capabilities = useChildCapabilities(props.parentId);
  const link = useQuery({ queryKey: childLinkKey(props.parentId, props.childId), queryFn: ({ signal }) => getChildLink(props.parentId, props.childId, signal), enabled: capabilities.control, retry: false, refetchInterval: 4000 });
  const state = props.detached ? "detached" : link.data?.control_state ?? "delegated";
  return <OriginContent {...props} state={state}>
    {link.error && <p role="alert" className="child-session-error">{childFailureText(link.error, props.language)}</p>}
    {capabilities.control && <ChildSessionControl parentId={props.parentId} childId={props.childId} state={state} language={props.language} onChanged={props.onRelationshipChanged} />}
  </OriginContent>;
}
function OriginContent({ parentId, parentTitle, readOnly = true, language, onOpenSession, state, children }: OriginProps & { state: ChildControlState; children?: React.ReactNode }) {
  const t = childSessionsText(language);
  const [failed, setFailed] = useState(false);
  const historical = state !== "delegated";
  return <aside className="child-session-origin"><span>{(historical ? t.sourceHistory : t.parent).replace("{parent}", parentTitle || parentId.slice(0, 8))} · {readOnly ? t.readOnly : t.worktree}{historical ? ` · ${t.controlStates[state]}` : ""}</span>
    <button type="button" onClick={() => { setFailed(false); try { onOpenSession(parentId); } catch { setFailed(true); } }}><ArrowLeft size={13} />{historical ? t.sourceBack : t.back}</button>
    {failed && <p role="alert">{t.parentMissing}</p>}{children}
  </aside>;
}
