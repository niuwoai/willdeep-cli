import { useRef, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Archive, CopyPlus, Pause, Play, Plus, Trash2 } from "lucide-react";
import { IconAction } from "./IconAction";
import type { Language, Messages } from "./i18n";
import { ScheduleForm } from "./ScheduleForm";
import { ScheduleHistory } from "./ScheduleHistory";
import { scheduleText } from "./scheduleMessages";
import { newSchedule, taskDraft, scheduleJson, scheduleWrite, ScheduleRequestError, type ScheduleDraft, type ScheduleList, type ScheduleView, type ScheduleWorkspace } from "./scheduleTypes";
import "./schedules.css";

export function ScheduleManager({ workspaces, workspace, language, messages, onOpenSession, onDirty }: {
  workspaces: ScheduleWorkspace[]; workspace: string; language: Language; messages: Messages; onOpenSession: (id: string, turnId?: string | null) => void; onDirty: (value: boolean) => void;
}) {
  const s = (key: Parameters<typeof scheduleText>[0]) => scheduleText(key, language);
  const client = useQueryClient();
  const list = useQuery({ queryKey: ["schedules"], queryFn: ({ signal }) => scheduleJson<ScheduleList>("/api/schedules", { signal }), refetchInterval: 5000 });
  const deleted = useQuery({ queryKey: ["deleted-schedules"], queryFn: ({ signal }) => scheduleJson<ScheduleView[]>("/api/schedules/deleted", { signal }), refetchInterval: 10_000 });
  const views = [...(list.data?.tasks ?? []), ...(deleted.data ?? [])];
  const [selection, setSelection] = useState<ScheduleView | null>(null);
  const [draft, setDraft] = useState<ScheduleDraft | null>(null);
  const [formKey, setFormKey] = useState(0);
  const [tab, setTab] = useState<"config" | "history">("config");
  const [search, setSearch] = useState("");
  const [filter, setFilter] = useState("all");
  const [error, setError] = useState("");
  const [pending, setPending] = useState(false);
  const pendingOperation = useRef<{ fingerprint: string; id: string } | null>(null);
  const [dirty, setDirty] = useState(false);
  const changeDirty = (value: boolean) => { setDirty(value); onDirty(value); };
  const leaveDraft = () => { if (dirty && !window.confirm(s("discard"))) return false; changeDirty(false); return true; };
  const [cleanupCount, setCleanupCount] = useState<number | null>(null);
  const writable = list.data?.writable ?? false;
  const current = views.find((view) => view.task.id === selection?.task.id) ?? selection;
  const state = (view: ScheduleView) => view.deleted ? "deleted" : view.task.archived_at ? "archived" : view.task.completed_at ? "completedTask" : view.task.enabled ? "enabled" : "paused";
  const select = (view: ScheduleView) => { if (!leaveDraft()) return; setSelection(view); setDraft(taskDraft(view.task)); setFormKey((key) => key + 1); setTab(view.deleted ? "history" : "config"); setCleanupCount(null); setError(""); };
  const refresh = async () => { await client.invalidateQueries({ queryKey: ["schedules"] }); await client.invalidateQueries({ queryKey: ["deleted-schedules"] }); await client.invalidateQueries({ queryKey: ["schedule-history"] }); await client.invalidateQueries({ queryKey: ["schedule-run"] }); };
  const operation = async (path: string, body: Record<string, unknown>) => {
    if (pending) return false;
    setPending(true); setError("");
    const fingerprint = JSON.stringify({ path, body });
    if (pendingOperation.current?.fingerprint !== fingerprint) pendingOperation.current = { fingerprint, id: crypto.randomUUID() };
    try { await scheduleWrite(path, { ...body, request_id: pendingOperation.current.id }); pendingOperation.current = null; await refresh(); return true; }
    catch (value) {
      if (value instanceof ScheduleRequestError && value.status < 500) pendingOperation.current = null;
      setError(`${s("error")}: ${value instanceof Error ? value.message : s("unknown")}`); return false;
    }
    finally { setPending(false); }
  };
  const action = async (value: string) => {
    if (!current) return;
    if (value === "archive" && !window.confirm(s("archiveConfirm"))) return;
    if (value === "delete" && !window.confirm(s("deleteConfirm"))) return;
    const succeeded = await operation(`/api/schedules/${current.task.id}/action`, { action: value, expected_revision: current.task.revision });
    if (value === "delete" && succeeded) { setSelection(null); setDraft(null); changeDirty(false); }
  };
  const cleanup = async (preview: boolean) => {
    if (!current || pending) return;
    if (!preview && !window.confirm(s("cleanupConfirm"))) return;
    setPending(true); setError("");
    try { const result = await scheduleWrite<{ run_ids: string[] }>(`/api/schedules/${current.task.id}/cleanup`, { preview, expected_revision: current.task.revision, request_id: crypto.randomUUID() }); setCleanupCount(preview ? result.run_ids.length : null); await refresh(); }
    catch (value) { setError(`${s("error")}: ${value instanceof Error ? value.message : s("unknown")}`); }
    finally { setPending(false); }
  };
  return <main className="schedule-manager"><header><div><h1>{s("title")}</h1><p>{s("intro")}</p></div><button className="schedule-new" disabled={!writable} onClick={() => { if (!leaveDraft()) return; setSelection(null); setDraft(newSchedule(workspace || workspaces[0]?.path || "", Intl.DateTimeFormat().resolvedOptions().timeZone)); setFormKey((key) => key + 1); setTab("config"); }}><Plus size={16} />{s("new")}</button></header>
    {!writable && list.data && <p className="schedule-notice">{s("readOnly")}</p>}
    {list.error && <p role="alert">{s("error")}: {list.error.message} <button onClick={() => void list.refetch()}>{s("refresh")}</button></p>}
    {error && <p role="alert" className="schedule-error">{error}</p>}
    <div className="schedule-layout"><aside><input aria-label={s("search")} placeholder={s("search")} value={search} onChange={(event) => setSearch(event.target.value)} /><select aria-label={s("all")} value={filter} onChange={(event) => setFilter(event.target.value)}>{(["all", "enabled", "paused", "archived", "completedTask", "deleted"] as const).map((key) => <option key={key} value={key}>{s(key)}</option>)}</select>
      {views.filter((view) => (filter === "all" || state(view) === filter) && `${view.task.name} ${view.task.workspace}`.toLocaleLowerCase().includes(search.toLocaleLowerCase())).map((view) => <button key={view.task.id} className={`schedule-task${selection?.task.id === view.task.id ? " active" : ""}`} onClick={() => select(view)}><strong>{view.task.name}</strong><span>{s(state(view))}</span><small>{view.task.workspace}</small><small>{view.active_run ? `${s("activeRun")}: ${s(view.active_run.status)}` : view.last_run && s(view.last_run.status)}</small></button>)}
      {list.data && views.length === 0 && <p>{s("empty")}</p>}
    </aside><section className="schedule-content">{draft ? <>
      {current && <><h2>{current.task.name}</h2><p>{s("next")}: {current.next_fire_at ? new Date(current.next_fire_at * 1000).toLocaleString(language) : s("noNext")}</p>
        {current.task.migration_reason && <p className="schedule-notice">{current.task.migration_reason === "legacy_plugin_disabled" ? s("migrationPaused") : current.task.migration_reason === "consecutive_failures" ? s("consecutivePaused") : current.task.migration_reason}</p>}
        <div className="schedule-actions schedule-task-actions"><button className="schedule-run-primary" disabled={!writable || pending || current.deleted} onClick={() => void operation(`/api/schedules/${current.task.id}/run`, {})}><Play size={14} />{s("runNow")}</button><IconAction label={current.task.enabled ? s("pause") : s("resume")} disabled={!writable || pending || current.deleted} onClick={() => void action(current.task.enabled ? "pause" : "resume")}>{current.task.enabled ? <Pause size={17} /> : <Play size={17} />}</IconAction><IconAction label={s("copy")} disabled={!writable} onClick={() => { if (!leaveDraft()) return; setSelection(null); setDraft({ ...taskDraft(current.task), name: s("copiedName").replace("{name}", current.task.name) }); setFormKey((key) => key + 1); }}><CopyPlus size={17} /></IconAction><IconAction label={s("archive")} disabled={!writable || pending || current.deleted} onClick={() => void action("archive")}><Archive size={17} /></IconAction><IconAction label={s("delete")} className="danger-action" disabled={!writable || pending || current.deleted} onClick={() => void action("delete")}><Trash2 size={17} /></IconAction></div>
        <nav className="schedule-tabs"><button aria-pressed={tab === "config"} onClick={() => setTab("config")}>{s("config")}</button><button aria-pressed={tab === "history"} onClick={() => setTab("history")}>{s("history")}</button></nav></>}
      <div hidden={tab !== "config"}><ScheduleForm key={formKey} initial={draft} id={selection?.task.id ?? null} revision={selection?.task.revision ?? 0} workspaces={workspaces} writable={writable && !current?.deleted} language={language} messages={messages} onDirty={changeDirty} onSaved={(view) => { setSelection(view); void refresh(); }} /></div>
      {current && tab === "history" && <><ScheduleHistory key={current.task.id} taskId={current.task.id} language={language} writable={writable && !pending && !current.deleted} onOperation={operation} onOpenSession={onOpenSession} /><details><summary>{s("maintenance")}</summary><button disabled={!writable || pending} onClick={() => void cleanup(true)}>{s("cleanup")}</button>{cleanupCount !== null && <p>{cleanupCount ? s("cleanupCount").replace("{count}", String(cleanupCount)) : s("cleanupEmpty")}</p>}{!!cleanupCount && <button disabled={!writable || pending} onClick={() => void cleanup(false)}>{s("cleanupApply")}</button>}</details></>}
    </> : <p>{s("select")}</p>}</section></div>
  </main>;
}
