import { useState } from "react";
import type { Language, Messages } from "./i18n";
import { WorkflowApiError, useWorkflowControl, useWorkflowHistory, useWorkflowRun, workflowActive, type WorkflowRun } from "./workflowApi";
import { WorkflowRunView, WorkflowStatus } from "./WorkflowRunView";
import { workflowText } from "./workflowMessages";

export function WorkflowError({ error, language }: { error: Error; language: Language }) {
  const t = workflowText(language);
  const label = error instanceof WorkflowApiError && error.status === 403
    ? /loopback|local connection/i.test(error.message) ? t.unavailable : t.readOnly : t.failed;
  return <p className="workflow-error" role="alert">{label}: {error.message}</p>;
}
export function WorkflowCard({ workflow, language, messages, canControl = false, compact = false, showHistory = true, initiallyExpanded = false }: {
  workflow: WorkflowRun; language: Language; messages: Messages; canControl?: boolean; compact?: boolean; showHistory?: boolean; initiallyExpanded?: boolean;
}) {
  const t = workflowText(language);
  const query = useWorkflowRun(workflow);
  const run = query.data;
  const control = useWorkflowControl(run.parent_session_id);
  const [historyOpen, setHistoryOpen] = useState(false);
  const [selected, setSelected] = useState<WorkflowRun | null>(null);
  const history = useWorkflowHistory(run.parent_session_id, run.definition_id, historyOpen);
  const active = workflowActive(run);
  const allowed = canControl && !(query.error instanceof WorkflowApiError && query.error.status === 403);
  const busy = control.isPending;
  return <section className={`workflow-card${compact ? " workflow-card-compact" : ""}`} aria-label={`${t.title}: ${run.spec.name}`} data-workflow-id={run.id}>
    <details className="workflow-disclosure" open={initiallyExpanded || undefined}>
      <summary><strong>{run.spec.name}</strong><WorkflowStatus status={run.status} t={t} /></summary>
      <div className="workflow-card-body">
        {query.error && <WorkflowError error={query.error} language={language} />}
        <div className="workflow-actions">
          <button type="button" onClick={() => void query.refetch()} disabled={query.isFetching}>{t.refresh}</button>
          {active && <button type="button" disabled={!allowed || busy || run.status === "stopping"} onClick={() => control.mutate({ action: "stop", runId: run.id })}>{t.stop}</button>}
          {run.status === "failed" && <button type="button" disabled={!allowed || busy} onClick={() => control.mutate({ action: "retry", runId: run.id }, { onSuccess: ({ workflow: next }) => setSelected(next) })}>{t.retry}</button>}
          {run.status === "interrupted" && <button type="button" disabled={!allowed || busy} onClick={() => control.mutate({ action: "resume", runId: run.id }, { onSuccess: ({ workflow: next }) => setSelected(next) })}>{t.resume}</button>}
          {showHistory && <button type="button" aria-expanded={historyOpen} onClick={() => setHistoryOpen((open) => !open)}>{t.history}</button>}
        </div>
        {!allowed && (active || run.status === "failed" || run.status === "interrupted") && <p className="workflow-note">{t.readOnly}</p>}
        {busy && <p role="status" className="workflow-note">{t.pending}</p>}
        {control.error && <WorkflowError error={control.error} language={language} />}
        {control.data && !busy && <p role="status" className="workflow-note">{t.received} {t.statuses[control.data.workflow.status]}</p>}
        <WorkflowRunView run={run} language={language} messages={messages} />
        {historyOpen && <div className="workflow-history" aria-label={t.history}>
          {history.isPending && <p role="status">{t.loading}</p>}
          {history.error && <><WorkflowError error={history.error} language={language} /><button type="button" onClick={() => void history.refetch()}>{t.refresh}</button></>}
          {history.data?.runs.length === 0 && <p className="workflow-note">{t.noHistory}</p>}
          {history.data?.runs.map((previous) => <button type="button" className="workflow-history-row" key={previous.id} onClick={() => setSelected(previous)}>
            <span>{previous.id}</span><WorkflowStatus status={previous.status} t={t} /><time dateTime={new Date(previous.created_at_ms).toISOString()}>{new Date(previous.created_at_ms).toLocaleString(language)}</time>
          </button>)}
        </div>}
        {selected && <div className="workflow-history-selection"><button type="button" className="workflow-close" onClick={() => setSelected(null)}>{t.close}</button>
          <WorkflowCard key={selected.id} workflow={selected} language={language} messages={messages} canControl={canControl} showHistory={false} compact initiallyExpanded />
        </div>}
      </div>
    </details>
  </section>;
}
