import { useRef, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { ChevronDown, Clock3 } from "lucide-react";
import type { Language, Messages } from "./i18n";
import { ScheduleForm } from "./ScheduleForm";
import { ScheduleHistory } from "./ScheduleHistory";
import { scheduleText } from "./scheduleMessages";
import { newSchedule, scheduleJson, scheduleWrite, ScheduleRequestError, type ScheduleDraft, type ScheduleList, type ScheduleWorkspace } from "./scheduleTypes";
import "./conversationSchedules.css";
import { useInspectorDismiss } from "./useInspectorDismiss";

export function ConversationSchedules({ sessionId, workspace, workspaces, language, messages, onOpenSession }: {
  sessionId: string; workspace: string; workspaces: ScheduleWorkspace[]; language: Language; messages: Messages;
  onOpenSession: (id: string, turnId?: string | null) => void;
}) {
  const s = (key: Parameters<typeof scheduleText>[0]) => scheduleText(key, language);
  const panel = useInspectorDismiss();
  const client = useQueryClient();
  const list = useQuery({ queryKey: ["schedules"], queryFn: ({ signal }) => scheduleJson<ScheduleList>("/api/schedules", { signal }), refetchInterval: 5000 });
  const tasks = list.data?.tasks.filter((view) => view.task.origin_session_id === sessionId) ?? [];
  const [selected, setSelected] = useState<string | null>(null);
  const [panelOpen, setPanelOpen] = useState(false);
  const [draft, setDraft] = useState<ScheduleDraft | null>(null);
  const [dirty, setDirty] = useState(false);
  const [pending, setPending] = useState(false);
  const [error, setError] = useState("");
  const request = useRef<{ fingerprint: string; id: string } | null>(null);
  const writable = list.data?.writable ?? false;
  const current = tasks.find((view) => view.task.id === selected);
  const leave = () => !dirty || window.confirm(s("discard"));
  const refresh = () => Promise.all(["schedules", "schedule-history", "schedule-run"].map((key) => client.invalidateQueries({ queryKey: [key] })));
  const operation = async (path: string, body: Record<string, unknown>) => {
    if (pending) return;
    setPending(true); setError("");
    const fingerprint = JSON.stringify({ path, body });
    if (request.current?.fingerprint !== fingerprint) request.current = { fingerprint, id: crypto.randomUUID() };
    try {
      await scheduleWrite(path, { ...body, request_id: request.current.id });
      request.current = null;
      await refresh();
    } catch (reason) {
      if (reason instanceof ScheduleRequestError && reason.status < 500) request.current = null;
      setError(reason instanceof Error ? reason.message : s("unknown"));
    } finally { setPending(false); }
  };
  return <aside className="conversation-schedules" aria-label={s("conversationTasks")}><details ref={panel} className="conversation-disclosure" open={panelOpen} onToggle={(event) => setPanelOpen(event.currentTarget.open)}>
    <summary aria-label={s("conversationTasks")} title={s("conversationTasks")}><Clock3 size={16} /><span>{s("conversationTasks")}</span>{tasks.length > 0 && <span className="task-count">{tasks.length}</span>}<ChevronDown size={13} /></summary>
    <div className="conversation-inspector schedule-inspector"><div className="inspector-heading"><h2>{s("conversationTasks")}</h2></div>
    <p>{s("conversationHint")}</p>
    <button type="button" disabled={!writable} onClick={() => {
      if (!leave()) return;
      setSelected(null); setDirty(false);
      setDraft({ ...newSchedule(workspace, Intl.DateTimeFormat().resolvedOptions().timeZone), origin_session_id: sessionId, continue_in_session: true });
    }}>{s("new")}</button>
    {list.isPending && <p role="status">{s("loadingTasks")}</p>}
    {(list.error || error) && <p role="alert">{s("error")}: {list.error?.message ?? error}<button onClick={() => void list.refetch()}>{s("refresh")}</button></p>}
    {list.data && !writable && <p>{s("readOnly")}</p>}
    {list.data && !tasks.length && <p>{s("empty")}</p>}
    {tasks.map((view) => <button type="button" key={view.task.id} className="conversation-schedule-task" aria-pressed={selected === view.task.id} onClick={() => {
      if (!leave()) return;
      setDraft(null); setDirty(false); setSelected(view.task.id);
    }}><strong>{view.task.name}</strong>
      <span>{s(view.task.completed_at ? "completedTask" : view.task.archived_at ? "archived" : view.task.enabled ? "enabled" : "paused")}</span>
      <small>{s("next")}: {view.next_fire_at ? new Date(view.next_fire_at * 1000).toLocaleString(language) : s("noNext")}</small>
      {view.last_run && <small>{s(view.last_run.status)}</small>}
    </button>)}
    {draft && <><button onClick={() => { if (leave()) { setDraft(null); setDirty(false); } }}>{messages.cancel}</button>
      <ScheduleForm initial={draft} id={null} revision={1} workspaces={workspaces.filter((item) => item.path === workspace)} writable={writable} language={language} messages={messages}
        onDirty={setDirty} onSaved={(view) => { setDraft(null); setDirty(false); setSelected(view.task.id); void refresh(); }} /></>}
    {current && <section><h3>{current.task.name}</h3>
      <button disabled={!writable || pending || !!current.task.completed_at || !!current.task.archived_at} onClick={() => void operation(`/api/schedules/${current.task.id}/action`, {
        action: current.task.enabled ? "pause" : "resume", expected_revision: current.task.revision,
      })}>{s(current.task.enabled ? "pause" : "resume")}</button>
      <ScheduleHistory key={current.task.id} taskId={current.task.id} language={language} writable={writable && !pending} onOperation={operation} onOpenSession={onOpenSession} />
    </section>}
    </div>
  </details></aside>;
}
