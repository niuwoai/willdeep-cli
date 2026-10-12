import { useEffect, useRef, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import type { Language, Messages } from "./i18n";
import { ScheduleTiming } from "./ScheduleTiming";
import { ScheduleOptions } from "./ScheduleOptions";
import { scheduleText } from "./scheduleMessages";
import { ScheduleRequestError, scheduleJson, scheduleWrite, type ScheduleDraft, type ScheduleView, type ScheduleWorkspace } from "./scheduleTypes";

export function ScheduleForm({ initial, id, revision, workspaces, writable, language, messages, onSaved, onDirty }: {
  initial: ScheduleDraft; id: string | null; revision: number; workspaces: ScheduleWorkspace[]; writable: boolean;
  language: Language; messages: Messages; onSaved: (view: ScheduleView) => void; onDirty: (dirty: boolean) => void;
}) {
  const s = (key: Parameters<typeof scheduleText>[0]) => scheduleText(key, language);
  const [draft, setDraft] = useState(initial);
  const [expected, setExpected] = useState(revision);
  const [error, setError] = useState("");
  const [saving, setSaving] = useState(false);
  const [uncertain, setUncertain] = useState(false);
  const [preview, setPreview] = useState<number[]>([]);
  const [onceTime, setOnceTime] = useState("");
  const onceAt = initial.schedule.kind === "onceAt" ? initial.schedule.at : null;
  const originalTime = useQuery({ queryKey: ["schedule-format-time", onceAt, initial.timezone], enabled: onceAt !== null,
    queryFn: () => scheduleWrite<string>("/api/schedules/format-time", { at: onceAt, timezone: initial.timezone }), staleTime: Infinity });
  useEffect(() => { if (originalTime.data) setOnceTime((value) => value || originalTime.data); }, [originalTime.data]);
  const pendingRequest = useRef<{ fingerprint: string; id: string; task: ScheduleDraft } | null>(null);
  const zones = useQuery({ queryKey: ["schedule-timezones"], queryFn: ({ signal }) => scheduleJson<string[]>("/api/schedules/timezones", { signal }), staleTime: Infinity });
  const prepared = async () => {
    if (draft.schedule.kind !== "onceAt") return draft;
    const unchanged = draft.schedule.at === onceAt && draft.timezone === initial.timezone && onceTime === originalTime.data;
    const at = onceTime && !unchanged ? await scheduleWrite<number>("/api/schedules/resolve-time", { local_datetime: onceTime, timezone: draft.timezone }) : draft.schedule.at;
    return { ...draft, schedule: { kind: "onceAt" as const, at } };
  };
  const failure = (value: unknown) => value instanceof ScheduleRequestError && value.status === 409 ? s("changed") : value instanceof ScheduleRequestError && value.status === 426 ? s("upgrade") : `${s("error")}: ${value instanceof Error ? value.message : s("unknown")}`;
  const save = async () => {
    setSaving(true); setError("");
    try {
      const task = uncertain && pendingRequest.current ? pendingRequest.current.task : await prepared();
      const fingerprint = JSON.stringify({ task, expected });
      if (!pendingRequest.current || pendingRequest.current.fingerprint !== fingerprint) pendingRequest.current = { fingerprint, id: crypto.randomUUID(), task };
      const view = await scheduleWrite<ScheduleView>(id ? `/api/schedules/${encodeURIComponent(id)}` : "/api/schedules", { task, expected_revision: expected, request_id: pendingRequest.current.id }, id ? "PUT" : "POST");
      pendingRequest.current = null;
      setUncertain(false); onDirty(false); setExpected(view.task.revision); onSaved(view);
    } catch (value) {
      const unknown = !(value instanceof ScheduleRequestError) || value.status >= 500;
      setUncertain(unknown && pendingRequest.current !== null);
      if (!unknown) pendingRequest.current = null;
      setError(failure(value));
    }
    finally { setSaving(false); }
  };
  const showPreview = async () => {
    setError("");
    try { const task = await prepared(); setPreview(await scheduleWrite<number[]>("/api/schedules/preview", { task_id: id, schedule: task.schedule, timezone: task.timezone, after: Math.floor(Date.now() / 1000), count: 5 })); }
    catch (value) { setError(failure(value)); }
  };
  const date = (at: number) => new Intl.DateTimeFormat(language, { dateStyle: "medium", timeStyle: "short", timeZone: "UTC" }).format(new Date(at * 1000));
  const change = (next: ScheduleDraft) => { setDraft(next); setPreview([]); onDirty(true); };
  return <form className="schedule-form" onSubmit={(event) => { event.preventDefault(); void save(); }}>
    <fieldset disabled={!writable || saving || uncertain} className="schedule-fields">
      <label>{s("name")}<input value={draft.name} maxLength={120} onChange={(event) => change({ ...draft, name: event.target.value })} required /></label>
      <label>{messages.workspace}<select value={draft.workspace} onChange={(event) => change({ ...draft, workspace: event.target.value })} required>
        <option value="">{messages.workspace}</option>{workspaces.map((workspace) => <option key={workspace.id} value={workspace.path}>{workspace.name}</option>)}
      </select></label>
      <label className="schedule-wide">{s("prompt")}<textarea rows={5} value={draft.prompt} onChange={(event) => change({ ...draft, prompt: event.target.value })} required /></label>
      <label className="schedule-wide">{s("goal")}<textarea rows={2} value={draft.goal ?? ""} onChange={(event) => change({ ...draft, goal: event.target.value || null })} /></label>
    </fieldset>
    <ScheduleTiming draft={draft} timezoneOptions={zones.data ?? []} onceTime={onceTime} onOnceTime={(value) => { setOnceTime(value); onDirty(true); }} onChange={change} language={language} disabled={!writable || saving || uncertain} />
    <ScheduleOptions draft={draft} onChange={change} language={language} messages={messages} disabled={!writable || saving || uncertain} />
    {uncertain && <p role="alert">{s("uncertain")}</p>}
    {error && <p role="alert" className="schedule-error">{error}</p>}
    <div className="schedule-actions"><button type="submit" className="schedule-primary" disabled={!writable || saving}>{saving ? s("saving") : s("save")}</button><button type="button" onClick={() => void showPreview()}>{s("preview")}</button></div>
    {preview.length > 0 && <ul className="schedule-preview">{preview.map((at) => <li key={at}>{date(at)} · {"UTC"}</li>)}</ul>}
  </form>;
}
