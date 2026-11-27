import { useEffect, useId, useRef, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { ArrowLeft, ChevronRight, Clock3, Plus } from "lucide-react";
import type { Language, Messages } from "./i18n";
import { createRequestId } from "./requestId";
import { ScheduleForm } from "./ScheduleForm";
import { ScheduleHistory } from "./ScheduleHistory";
import { scheduleText } from "./scheduleMessages";
import { newSchedule, scheduleJson, scheduleWrite, ScheduleRequestError, type ScheduledTask, type ScheduleDraft, type ScheduleList, type ScheduleView, type ScheduleWorkspace } from "./scheduleTypes";
import "./conversationSchedules.css";
import { useInspectorDismiss } from "./useInspectorDismiss";

type PanelView = { page: "list" } | { page: "create"; draft: ScheduleDraft } | { page: "detail"; taskId: string };
const SECONDS_TO_MILLISECONDS = 1000;
const SCHEDULE_QUERY_KEY = ["schedules"] as const;

function ScheduleStatus({ task, language }: { task: ScheduledTask; language: Language }) {
  const status = task.archived_at != null ? "archived" : task.completed_at != null ? "completedTask" : task.enabled ? "enabled" : "paused";
  return <span className="conversation-schedule-status" data-status={status}>{scheduleText(status, language)}</span>;
}

function ScheduleNextRun({ at, language }: { at: number | null; language: Language }) {
  const date = at === null ? null : new Date(at * SECONDS_TO_MILLISECONDS);
  return <span className="conversation-schedule-next">
    <span>{scheduleText("next", language)}</span>
    {date && Number.isFinite(date.getTime()) ? <time dateTime={date.toISOString()}>
      {new Intl.DateTimeFormat(language, { dateStyle: "medium", timeStyle: "short" }).format(date)}
    </time> : <span>{scheduleText("noNext", language)}</span>}
  </span>;
}

export function ConversationSchedules({ sessionId, workspace, workspaces, language, messages, onOpenSession }: {
  sessionId: string; workspace: string; workspaces: ScheduleWorkspace[]; language: Language; messages: Messages;
  onOpenSession: (id: string, turnId?: string | null) => void;
}) {
  const s = (key: Parameters<typeof scheduleText>[0]) => scheduleText(key, language);
  const panel = useInspectorDismiss();
  const panelId = useId();
  const titleId = useId();
  const heading = useRef<HTMLHeadingElement>(null);
  const client = useQueryClient();
  const list = useQuery({ queryKey: SCHEDULE_QUERY_KEY, queryFn: ({ signal }) => scheduleJson<ScheduleList>("/api/schedules", { signal }), refetchInterval: 5000 });
  const tasks = list.data?.tasks.filter((view) => view.task.origin_session_id === sessionId) ?? [];
  const [view, setView] = useState<PanelView>({ page: "list" });
  const [dirty, setDirty] = useState(false);
  const [pending, setPending] = useState(false);
  const [error, setError] = useState("");
  const [saved, setSaved] = useState(false);
  const request = useRef<{ fingerprint: string; id: string } | null>(null);
  const writable = list.data?.writable ?? false;
  const current = view.page === "detail" ? tasks.find(({ task }) => task.id === view.taskId) : undefined;
  const count = new Intl.NumberFormat(language).format(tasks.length);
  const countLabel = s("conversationTaskCount").replace("{count}", count);
  const summaryLabel = list.data ? `${s("conversationTasks")} (${countLabel})` : s("conversationTasks");

  useEffect(() => {
    if (panel.current?.open) heading.current?.focus();
  }, [panel, view.page]);

  const navigate = (next: PanelView) => {
    if (dirty && !window.confirm(s("discard"))) return;
    setDirty(false);
    setError("");
    setSaved(false);
    setView(next);
  };
  const refresh = () => Promise.all([SCHEDULE_QUERY_KEY, ["schedule-history"], ["schedule-run"]].map((queryKey) => client.invalidateQueries({ queryKey })));
  const onSaved = (created: ScheduleView) => {
    // Seed the list so the saved task opens immediately while the server list refreshes.
    client.setQueryData<ScheduleList>(SCHEDULE_QUERY_KEY, (prior) => prior && ({
      ...prior,
      tasks: [...prior.tasks.filter(({ task }) => task.id !== created.task.id), created],
    }));
    setDirty(false);
    setSaved(true);
    setView({ page: "detail", taskId: created.task.id });
    void refresh();
  };
  const operation = async (path: string, body: Record<string, unknown>) => {
    if (pending) return;
    setPending(true);
    setError("");
    setSaved(false);
    const fingerprint = JSON.stringify({ path, body });
    if (request.current?.fingerprint !== fingerprint) request.current = { fingerprint, id: createRequestId() };
    try {
      await scheduleWrite(path, { ...body, request_id: request.current.id });
      request.current = null;
      void refresh();
    } catch (reason) {
      if (reason instanceof ScheduleRequestError && reason.status < 500) {
        request.current = null;
        if (reason.status === 409) void list.refetch();
      }
      setError(reason instanceof Error ? reason.message : s("unknown"));
    } finally {
      setPending(false);
    }
  };

  return <aside className="conversation-schedules" aria-label={s("conversationTasks")}>
    <details ref={panel} className="conversation-disclosure">
      <summary aria-label={summaryLabel} aria-controls={panelId} title={summaryLabel}><Clock3 size={16} aria-hidden="true" /></summary>
      <div id={panelId} role="region" aria-labelledby={titleId} className="conversation-inspector schedule-inspector" data-view={view.page}>
        <header className="conversation-schedule-header">
          <div className="conversation-schedule-heading">
            <Clock3 size={18} aria-hidden="true" />
            <h2 id={titleId} ref={view.page === "list" ? heading : undefined} tabIndex={-1}>{s("conversationTasks")}</h2>
            {list.data && <span className="conversation-schedule-count" aria-hidden="true">{count}</span>}
          </div>
          {view.page === "list" && <button type="button" className="conversation-schedule-primary" disabled={!writable} onClick={() => navigate({
            page: "create", draft: { ...newSchedule(workspace, Intl.DateTimeFormat().resolvedOptions().timeZone), origin_session_id: sessionId, continue_in_session: true },
          })}><Plus size={15} aria-hidden="true" />{s("new")}</button>}
          {view.page === "detail" && <button type="button" className="conversation-schedule-back" disabled={pending} onClick={() => navigate({ page: "list" })}>
            <ArrowLeft size={14} aria-hidden="true" />{s("conversationBackToList")}
          </button>}
        </header>
        {(list.error || error || (list.data && !writable) || saved) && <div className="conversation-schedule-notices">
          {list.error && <div className="conversation-schedule-notice" data-tone="error">
            <p role="alert">{s("error")}: {list.error.message}</p>
            <button type="button" disabled={list.isFetching} onClick={() => void list.refetch()}>{s("refresh")}</button>
          </div>}
          {error && <p className="conversation-schedule-notice" data-tone="error" role="alert">{s("error")}: {error}</p>}
          {list.data && !writable && <p className="conversation-schedule-notice" role="status">{s("readOnly")}</p>}
          {saved && <p className="conversation-schedule-notice" role="status">{s("saved")}</p>}
        </div>}
        {view.page === "list" && <div className="conversation-schedule-scroll conversation-schedule-list">
          <p className="conversation-schedule-hint">{s("conversationHint")}</p>
          {list.isPending && <p className="conversation-schedule-empty" role="status">{s("loadingTasks")}</p>}
          {list.data && !tasks.length && <p className="conversation-schedule-empty">{s("empty")}</p>}
          {tasks.length > 0 && <ul>
            {tasks.map(({ task, next_fire_at, last_run }) => <li key={task.id}>
              <button type="button" className="conversation-schedule-task" onClick={() => navigate({ page: "detail", taskId: task.id })}>
                <span className="conversation-schedule-task-name">{task.name}</span>
                <ScheduleStatus task={task} language={language} />
                <ChevronRight className="conversation-schedule-arrow" size={16} aria-hidden="true" />
                <ScheduleNextRun at={next_fire_at} language={language} />
                {last_run && <span className="conversation-schedule-last">{s("conversationLastRun")}: {s(last_run.status)}</span>}
              </button>
            </li>)}
          </ul>}
        </div>}
        {view.page === "create" && <>
          <h3 className="conversation-schedule-view-title" ref={heading} tabIndex={-1}>{s("new")}</h3>
          <ScheduleForm initial={view.draft} id={null} revision={1} workspaces={workspaces.filter((item) => item.path === workspace)} writable={writable} language={language} messages={messages}
            conversationPanel={{ onCancel: () => navigate({ page: "list" }) }} onDirty={setDirty} onSaved={onSaved} />
        </>}
        {view.page === "detail" && <section className="conversation-schedule-scroll conversation-schedule-detail">
          <h3 ref={heading} tabIndex={-1}>{current ? current.task.name : s("conversationTaskUnavailable")}</h3>
          {current && <>
            <div className="conversation-schedule-detail-meta">
              <ScheduleStatus task={current.task} language={language} />
              <ScheduleNextRun at={current.next_fire_at} language={language} />
              {current.last_run && <span className="conversation-schedule-last">{s("conversationLastRun")}: {s(current.last_run.status)}</span>}
            </div>
            <button type="button" disabled={!writable || pending || current.task.completed_at != null || current.task.archived_at != null} onClick={() => void operation(`/api/schedules/${encodeURIComponent(current.task.id)}/action`, {
              action: current.task.enabled ? "pause" : "resume", expected_revision: current.task.revision,
            })}>{s(current.task.enabled ? "pause" : "resume")}</button>
            <ScheduleHistory key={current.task.id} taskId={current.task.id} language={language} writable={writable && !pending} onOperation={operation} onOpenSession={onOpenSession} />
          </>}
        </section>}
      </div>
    </details>
  </aside>;
}
