import { useState } from "react";
import { useInfiniteQuery, useQuery } from "@tanstack/react-query";
import type { Language } from "./i18n";
import { scheduleText, scheduleReason } from "./scheduleMessages";
import { runStatuses, runIsActive, runCanRetry, scheduleJson, type ScheduleHistory as History, type ScheduleRun } from "./scheduleTypes";

export function ScheduleHistory({ taskId, language, writable, onOperation, onOpenSession }: {
  taskId: string; language: Language; writable: boolean;
  onOperation: (path: string, body: Record<string, unknown>) => Promise<unknown>;
  onOpenSession: (id: string, turnId?: string | null) => void;
}) {
  const s = (key: Parameters<typeof scheduleText>[0]) => scheduleText(key, language);
  const [status, setStatus] = useState("");
  const [selected, setSelected] = useState<string | null>(null);
  const base = `/api/schedules/${encodeURIComponent(taskId)}`;
  const history = useInfiniteQuery({ queryKey: ["schedule-history", taskId, status], initialPageParam: "",
    queryFn: ({ pageParam, signal }) => scheduleJson<History>(`${base}/history?${new URLSearchParams({ ...(pageParam ? { before: pageParam } : {}), ...(status ? { status } : {}), limit: "25" })}`, { signal }),
    getNextPageParam: (page) => page.next_cursor ?? undefined, refetchInterval: 5000 });
  const detail = useQuery({ queryKey: ["schedule-run", taskId, selected], enabled: selected !== null,
    queryFn: ({ signal }) => scheduleJson<ScheduleRun>(`${base}/history/${encodeURIComponent(selected ?? "")}`, { signal }), refetchInterval: 5000 });
  const date = (at: number | null) => at === null ? s("unknown") : new Intl.DateTimeFormat(language, { dateStyle: "medium", timeStyle: "medium" }).format(new Date(at * 1000));
  const run = detail.data;
  return <section className="schedule-history">
    <label>{s("history")}<select value={status} onChange={(event) => setStatus(event.target.value)}>
      <option value="">{s("all")}</option>{runStatuses.map((item) => <option key={item} value={item}>{s(item)}</option>)}
    </select></label>
    {history.error && <p role="alert">{s("error")}: {history.error.message}</p>}
    <div className="schedule-run-list">{history.data?.pages.flatMap((page) => page.runs).map((item) => <button key={item.id} type="button" className={selected === item.id ? "active" : ""} onClick={() => setSelected(item.id)}>
      <span>{date(item.scheduled_at)}</span><strong>{s(item.status)}</strong><span>{s(item.trigger)}</span>
    </button>)}</div>
    {history.data?.pages[0]?.runs.length === 0 && <p>{s("noRuns")}</p>}
    {history.hasNextPage && <button disabled={history.isFetchingNextPage} onClick={() => void history.fetchNextPage()}>{s("more")}</button>}
    {detail.error && <p role="alert">{s("error")}: {detail.error.message}</p>}
    {run && <article className="schedule-run-detail"><h3>{s("runDetails")}</h3>
      <dl><dt>{s("history")}</dt><dd>{s(run.status)}</dd><dt>{s("trigger")}</dt><dd>{s(run.trigger)}</dd>
        <dt>{s("runAt")}</dt><dd>{date(run.scheduled_at)}</dd><dt>{s("started")}</dt><dd>{date(run.started_at)}</dd><dt>{s("finished")}</dt><dd>{date(run.finished_at)}</dd>
        <dt>{s("elapsed")}</dt><dd>{run.started_at !== null && run.finished_at !== null ? `${Math.max(0, run.finished_at - run.started_at)} s` : s("unknown")}</dd>
        <dt>{s("model")}</dt><dd>{run.snapshot.model ?? s("unknown")}</dd>
        <dt>{s("profile")}</dt><dd>{run.snapshot.profile ?? s("unknown")}</dd>
        <dt>{s("usage")}</dt><dd>{run.usage ? `${s("inputTokens")} ${run.usage.input_tokens} · ${s("outputTokens")} ${run.usage.output_tokens} · ${s("totalTokens")} ${run.usage.total_tokens}` : s("unknownUsage")}</dd>
      </dl><p>{run.verification === "passed" ? s("verified") : run.verification === "failed" ? s("verificationFailed") : s("unverified")}</p>
      {run.reason && <p>{s("reason")}: {scheduleReason(run.reason, language)}</p>}
      <h4>{s("result")}</h4><pre>{run.result ?? s("outputUnavailable")}</pre>
      <details><summary>{s("snapshot")}</summary><pre>{JSON.stringify(run.snapshot, null, 2)}</pre></details>
      <div className="schedule-actions">{run.session_id && <button onClick={() => onOpenSession(run.session_id!, runIsActive(run.status) ? run.turn_id : null)}>{s("openSession")}</button>}
        {writable && runIsActive(run.status) && <button onClick={() => { if (window.confirm(s("stopConfirm"))) void onOperation(`${base}/history/${run.id}/stop`, {}); }}>{s("stopRun")}</button>}
        {writable && runCanRetry(run.status) && <button title={s("retryHint")} onClick={() => void onOperation(`${base}/run`, { retry_of: run.id })}>{s("retryRun")}</button>}
      </div>
    </article>}
  </section>;
}
