import { useState } from "react";
import type { Language, Messages } from "./i18n";
import { useWorkflowList, workflowActive } from "./workflowApi";
import { WorkflowCard, WorkflowError } from "./WorkflowCard";
import { workflowText } from "./workflowMessages";
import "./Workflow.css";

const RECENT_RUNS = 3;
export function WorkflowSidebar({ parentSessionId, language, messages, canControl }: {
  parentSessionId: string; language: Language; messages: Messages; canControl: boolean;
}) {
  const t = workflowText(language);
  const query = useWorkflowList(parentSessionId);
  const [open, setOpen] = useState(false);
  const [all, setAll] = useState(false);
  const runs = (query.data?.runs ?? []).filter((run) => run.parent_session_id === parentSessionId)
    .sort((a, b) => b.created_at_ms - a.created_at_ms);
  const visible = all ? runs : runs.filter((run, index) => workflowActive(run) || index < RECENT_RUNS);
  return <section className="workflow-sidebar runtime-section" aria-label={t.title}>
    <button type="button" className="runtime-section-head workflow-section-toggle" aria-expanded={open} onClick={() => setOpen((current) => !current)}><span>{t.title}</span><span className="runtime-section-count">{runs.length}</span></button>
    {open && <div className="runtime-list">
      {!parentSessionId ? <p className="workflow-note">{t.chooseSession}</p> : <>
        <div className="workflow-actions"><button type="button" disabled={query.isFetching} onClick={() => void query.refetch()}>{t.refresh}</button></div>
        {query.isPending && <p className="workflow-note" role="status">{t.loading}</p>}
        {query.error && <WorkflowError error={query.error} language={language} />}
        {query.data && runs.length === 0 && <p className="workflow-note">{t.empty}</p>}
        {visible.map((run) => <WorkflowCard key={run.id} workflow={run} language={language} messages={messages} canControl={canControl} compact />)}
        {runs.length > RECENT_RUNS && <div className="workflow-actions"><button type="button" aria-expanded={all} onClick={() => setAll((value) => !value)}>{all ? t.close : t.history}</button></div>}
      </>}
    </div>}
  </section>;
}
