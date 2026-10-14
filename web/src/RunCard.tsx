import { useId, useState } from "react";
import type { Messages } from "./i18n";
import { formatStepElapsed, type RunStep } from "./runActivity";

function StepRow({ step, expanded = false }: { step: RunStep; expanded?: boolean }) {
  return <div className={`run-step ${step.status}${expanded ? " run-step-expanded" : ""}`}>
    <span className="step-dot" aria-hidden="true" />
    <span className="run-step-label">{step.label}</span>
    {step.detail && <span className="run-step-detail" title={step.detail}>{step.detail}</span>}
    {step.elapsedMs !== undefined && <span className="run-step-elapsed">{formatStepElapsed(step.elapsedMs)}</span>}
  </div>;
}

/** 连续调用默认一行汇总；运行中或失败时最多再显示一行，展开后保留全部原始步骤。 */
export function RunCard({ steps, messages: t }: { steps: RunStep[]; messages: Messages }) {
  const [expanded, setExpanded] = useState(false);
  const detailsId = useId();
  if (steps.length === 0) return null;
  if (steps.length === 1) return <div className="run-card"><StepRow step={steps[0]} /></div>;

  const tools = steps.filter(step => !step.id.startsWith("turn-"));
  const failed = tools.filter(step => step.status === "failed");
  const active = [...steps].reverse().find(step => step.status === "active");
  const preview = active ?? failed[failed.length - 1];
  const status = active ? "active" : failed.length ? "failed" : "done";
  const label = `${tools.length ? t.toolActivitySummary.replace("{count}", String(tools.length)) : t.foldedSteps.replace("{count}", String(steps.length))}${failed.length ? t.foldedFailed.replace("{count}", String(failed.length)) : ""}`;
  return <div className="run-card">
    <div className={`run-step run-summary ${status}`}>
      <span className="step-dot" aria-hidden="true" />
      <span className="run-summary-label">{label}</span>
      <button type="button" className="run-toggle" aria-expanded={expanded} aria-controls={detailsId} onClick={() => setExpanded(!expanded)}>{expanded ? t.collapse : t.expand}</button>
    </div>
    {!expanded && preview && <StepRow step={preview} />}
    {expanded && <div id={detailsId} className="run-details">{steps.map(step => <StepRow key={step.id} step={step} expanded />)}</div>}
  </div>;
}
