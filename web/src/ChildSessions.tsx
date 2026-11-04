import { useRef, useState } from "react";
import { useInfiniteQuery, useMutation, useQueryClient } from "@tanstack/react-query";
import { ArrowLeft, ArrowUpRight, GitBranch, RefreshCw } from "lucide-react";
import type { Language } from "./i18n";
import { childSessionsText } from "./childSessionsMessages";
import { ChildSessionFailure, childTaskRetryable, childTaskTerminal, continueChildTask, listChildTasks, retryChildTask, startChildTask, stopChildTask, type ChildTask, type StartChildOptions } from "./childSessionsApi";
import "./childSessions.css";

const BRIEF_LIMIT = 16_000;
type Draft = {
  title: string; prompt: string;
  context: "fresh" | "fork";
  throughTurnId: string;
  workspaceMode: "shared_read_only" | "worktree";
  worktreeBase: "head" | "snapshot";
  model: string;
};
const emptyDraft = (): Draft => ({ title: "", prompt: "", context: "fresh", throughTurnId: "", workspaceMode: "shared_read_only", worktreeBase: "head", model: "" });
const drafts = new Map<string, Draft>();
type Action = { kind: "start"; options: StartChildOptions } | { kind: "continue"; child: string; prompt: string } | { kind: "retry"; task: ChildTask } | { kind: "stop"; task: ChildTask };

export function ChildSessions({ sessionId, language, onOpenSession }: {
  sessionId: string; language: Language; onOpenSession: (id: string) => void;
}) {
  const t = childSessionsText(language);
  const cache = useQueryClient();
  const queryKey = ["child-session-tasks", sessionId];
  const tasks = useInfiniteQuery({
    queryKey, initialPageParam: null as string | null,
    queryFn: ({ pageParam, signal }) => listChildTasks(sessionId, pageParam, signal),
    getNextPageParam: (page) => page.next_cursor ?? undefined,
    refetchInterval: 4000, retry: false,
  });
  const [draft, setDraft] = useState<Draft>(() => drafts.get(sessionId) ?? emptyDraft());
  const [continuation, setContinuation] = useState<{ child: string; prompt: string } | null>(null);
  const [notice, setNotice] = useState(false);
  const identities = useRef(new Map<string, string>());
  const update = (next: Draft) => { drafts.set(sessionId, next); setDraft(next); setNotice(false); };
  const mutation = useMutation({
    mutationFn: (action: Action) => {
      const fingerprint = JSON.stringify([sessionId, action]);
      let id = identities.current.get(fingerprint);
      if (!id) { id = crypto.randomUUID(); identities.current.set(fingerprint, id); }
      switch (action.kind) {
        case "start": return startChildTask(sessionId, action.options, id);
        case "continue": return continueChildTask(sessionId, action.child, action.prompt, id);
        case "retry": return retryChildTask(sessionId, action.task, id);
        case "stop": return stopChildTask(sessionId, action.task.assignment_id, id);
      }
    },
    onSuccess: async (_, action) => {
      if (action.kind === "start") { drafts.delete(sessionId); setDraft(emptyDraft()); setNotice(true); }
      if (action.kind === "continue") setContinuation(null);
      identities.current.delete(JSON.stringify([sessionId, action]));
      await cache.invalidateQueries({ queryKey });
    },
  });
  const failure = mutation.error ?? tasks.error;
  const unavailable = failure instanceof ChildSessionFailure && [403, 404, 501].includes(failure.status);
  const errorMessage = failure instanceof ChildSessionFailure
    ? failure.status === 403 ? t.forbidden : [404, 501].includes(failure.status) ? t.unsupported : `${t.error} ${failure.message}`
    : t.error;
  const allTasks = tasks.data?.pages.flatMap((page) => page.tasks) ?? [];
  const uniqueTasks = [...new Map(allTasks.map((task) => [task.assignment_id, task])).values()].sort((a, b) => b.created_at - a.created_at);
  const pending = mutation.isPending;
  const validPrompt = (prompt: string) => !!prompt.trim() && Array.from(prompt.trim()).length <= BRIEF_LIMIT;
  return <section className="child-sessions" aria-label={t.title}>
    <header className="child-sessions-heading"><h2><GitBranch size={16} />{t.title}</h2><button type="button" disabled={tasks.isFetching} aria-label={t.refresh} title={t.refresh} onClick={() => void tasks.refetch()}><RefreshCw size={14} /></button></header>
    {tasks.isPending && <p role="status">{t.loading}</p>}
    {failure && <p className="child-session-error" role="alert">{errorMessage}</p>}
    {!unavailable && <details className="child-create"><summary>{t.create}</summary>
      <p className="child-session-hint">{t.hint}</p>
      <form onSubmit={(event) => {
        event.preventDefault();
        if (pending || !draft.title.trim() || !validPrompt(draft.prompt)) return;
        mutation.mutate({ kind: "start", options: {
          title: draft.title.trim(), prompt: draft.prompt.trim(),
          context: draft.context,
          throughTurnId: draft.context === "fork" && draft.throughTurnId.trim() ? draft.throughTurnId.trim() : null,
          workspaceMode: draft.workspaceMode,
          worktreeBase: draft.workspaceMode === "worktree" ? draft.worktreeBase : "head",
          model: draft.model.trim() || null,
        } });
      }}>
        <label>{t.titleLabel}<input value={draft.title} maxLength={200} disabled={pending} onChange={(event) => update({ ...draft, title: event.target.value })} /></label>
        <label>{t.brief}<textarea value={draft.prompt} disabled={pending} placeholder={t.placeholder} onChange={(event) => update({ ...draft, prompt: event.target.value })} /></label>
        <div className="child-options">
          <label>{t.contextLabel}<select value={draft.context} disabled={pending} onChange={(event) => update({ ...draft, context: event.target.value === "fork" ? "fork" : "fresh" })}>
            <option value="fresh">{t.contextFresh}</option><option value="fork">{t.contextFork}</option>
          </select></label>
          {draft.context === "fork" && <label>{t.forkTurnLabel}<input value={draft.throughTurnId} disabled={pending} placeholder={t.forkTurnPlaceholder} onChange={(event) => update({ ...draft, throughTurnId: event.target.value })} /></label>}
          <label>{t.workspaceLabel}<select value={draft.workspaceMode} disabled={pending} onChange={(event) => update({ ...draft, workspaceMode: event.target.value === "worktree" ? "worktree" : "shared_read_only" })}>
            <option value="shared_read_only">{t.workspaceReadOnly}</option><option value="worktree">{t.workspaceWorktree}</option>
          </select></label>
          {draft.workspaceMode === "worktree" && <label>{t.worktreeBaseLabel}<select value={draft.worktreeBase} disabled={pending} onChange={(event) => update({ ...draft, worktreeBase: event.target.value === "snapshot" ? "snapshot" : "head" })}>
            <option value="head">{t.worktreeHead}</option><option value="snapshot">{t.worktreeSnapshot}</option>
          </select></label>}
          <label>{t.modelLabel}<input value={draft.model} maxLength={256} disabled={pending} placeholder={t.modelPlaceholder} onChange={(event) => update({ ...draft, model: event.target.value })} /></label>
        </div>
        {draft.workspaceMode === "worktree" && <p className="child-session-hint">{t.worktreeHint}</p>}
        {draft.model.trim() && <p className="child-session-hint">{t.modelHint}</p>}
        <footer><span>{Array.from(draft.prompt).length}/{BRIEF_LIMIT}</span><button type="submit" disabled={pending || !draft.title.trim() || !validPrompt(draft.prompt)}>{pending ? t.working : t.start}</button></footer>
      </form>
    </details>}
    {notice && <p role="status">{t.created}</p>}
    {!tasks.isPending && !failure && !uniqueTasks.length && <p className="child-session-empty">{t.empty}</p>}
    <ol className="child-task-list">{uniqueTasks.map((task) => {
      const terminal = childTaskTerminal(task.status);
      const result = task.result;
      const childBusy = uniqueTasks.some((other) => other.child_session_id === task.child_session_id && !childTaskTerminal(other.status));
      return <li className={`child-task ${terminal ? "terminal" : "active"}`} key={task.assignment_id}>
        <header><strong>{task.title}</strong><span className={`child-task-state ${task.status}`}>{t.statuses[task.status] ?? t.unknown}</span></header>
        <p className="child-session-scope">{task.workspace_mode === "worktree" ? t.worktree : t.readOnly}{task.context === "fork" ? ` · ${t.fork}` : ""}{task.model ? ` · ${task.model}` : ""}</p>
        <details className="child-task-brief"><summary>{t.briefToggle}</summary><p>{task.prompt}</p></details>
        {task.error && <p className="child-session-error">{task.error}</p>}
        {result && <details className="child-task-result"><summary>{t.result}<span>{result.superseded_by ? t.superseded : result.consumed_at != null ? t.consumed : t.pending}</span></summary>
          <p>{result.report || result.summary || t.noReport}</p>{!result.report_complete && <p className="child-session-hint">{t.incomplete}</p>}
          {result.worktree && <p className="child-session-scope">{t.worktreeBranch}: {result.worktree.branch} · {t.worktreeBase}: {result.worktree.base_commit}{result.worktree.base === "snapshot" && result.worktree.untracked_files > 0 ? ` · ${result.worktree.untracked_files} ${t.worktreeUntracked}` : ""}</p>}
          {result.worktree?.base === "head" && result.worktree.parent_dirty && <p className="child-session-hint">{t.worktreeDirty}</p>}
        </details>}
        {terminal && !result && <p className="child-session-hint">{t.noReport}</p>}
        <div className="child-task-actions"><button type="button" onClick={() => onOpenSession(task.child_session_id)}>{t.open}<ArrowUpRight size={13} /></button>
          {!terminal && <button type="button" disabled={pending} onClick={() => mutation.mutate({ kind: "stop", task })}>{t.stop}</button>}
          {childTaskRetryable(task) && <button type="button" disabled={pending || childBusy} onClick={() => mutation.mutate({ kind: "retry", task })}>{t.retry}</button>}
          {terminal && !result?.superseded_by && <button type="button" disabled={pending || childBusy} onClick={() => setContinuation({ child: task.child_session_id, prompt: continuation?.child === task.child_session_id ? continuation.prompt : "" })}>{t.continue}</button>}
        </div>
        {continuation?.child === task.child_session_id && terminal && !result?.superseded_by && uniqueTasks.find((other) => other.child_session_id === task.child_session_id)?.assignment_id === task.assignment_id && <form className="child-continue" onSubmit={(event) => { event.preventDefault(); if (!pending && validPrompt(continuation.prompt)) mutation.mutate({ kind: "continue", child: continuation.child, prompt: continuation.prompt.trim() }); }}>
          <label>{t.continueLabel}<textarea value={continuation.prompt} disabled={pending} onChange={(event) => setContinuation({ ...continuation, prompt: event.target.value })} /></label>
          <footer><button type="button" disabled={pending} onClick={() => setContinuation(null)}>{t.cancel}</button><button type="submit" disabled={pending || !validPrompt(continuation.prompt)}>{t.submit}</button></footer>
        </form>}
      </li>;
    })}</ol>
    {tasks.hasNextPage && <button className="child-more" type="button" disabled={tasks.isFetchingNextPage} onClick={() => void tasks.fetchNextPage()}>{t.more}</button>}
  </section>;
}

export function ChildSessionOrigin({ parentId, parentTitle, readOnly = true, language, onOpenSession }: {
  parentId: string; parentTitle?: string; readOnly?: boolean; language: Language; onOpenSession: (id: string) => void;
}) {
  const t = childSessionsText(language);
  const [failed, setFailed] = useState(false);
  return <aside className="child-session-origin"><span>{t.parent.replace("{parent}", parentTitle || parentId.slice(0, 8))} · {readOnly ? t.readOnly : t.worktree}</span>
    <button type="button" onClick={() => { setFailed(false); try { onOpenSession(parentId); } catch { setFailed(true); } }}><ArrowLeft size={13} />{t.back}</button>
    {failed && <p role="alert">{t.parentMissing}</p>}
  </aside>;
}
