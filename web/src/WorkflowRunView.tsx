import { CheckCircle2, Circle, LoaderCircle, Square, XCircle } from "lucide-react";
import type { Language, Messages } from "./i18n";
import { Markdown } from "./Markdown";
import { markdownCopy } from "./markdownCopy";
import { workflowText, type WorkflowMessages } from "./workflowMessages";
import type { JsonValue, WorkflowNode, WorkflowNodeInstance, WorkflowNodeStatus, WorkflowRun } from "./workflowApi";
import "./Workflow.css";

function workflowDuration(start: number | null, end: number | null, t: WorkflowMessages) {
  return start === null ? t.unknown : `${(Math.max(0, (end ?? Date.now()) - start) / 1000).toFixed(1)} ${t.seconds}`;
}
function Timestamp({ value, language, fallback }: { value: number | null; language: Language; fallback: string }) {
  return value === null ? <>{fallback}</> : <time dateTime={new Date(value).toISOString()}>{new Date(value).toLocaleString(language)}</time>;
}
export function WorkflowStatus({ status, t }: { status: WorkflowNodeStatus; t: WorkflowMessages }) {
  const Icon = status === "completed" ? CheckCircle2 : status === "running" || status === "stopping" ? LoaderCircle
    : status === "failed" || status === "blocked" ? XCircle : status === "cancelled" || status === "interrupted" ? Square : Circle;
  return <span className="workflow-status" data-status={status}><Icon size={12} aria-hidden="true" />{t.statuses[status]}</span>;
}
function ValueDetails({ label, value }: { label: string; value: JsonValue }) {
  return value === null ? null : <details className="workflow-value"><summary>{label}</summary><pre>{typeof value === "string" ? value : JSON.stringify(value, null, 2)}</pre></details>;
}
function Report({ label, content, messages }: { label: string; content: string | null; messages: Messages }) {
  return content === null ? null : <details className="workflow-value"><summary>{label}</summary><Markdown content={content} copy={markdownCopy(messages)} /></details>;
}
function NodeUsage({ node, t, language }: { node: WorkflowNodeInstance; t: WorkflowMessages; language: Language }) {
  const agent = node.agent_run;
  // Reused work keeps the original AgentRun, while this attempt's usage remains distinct.
  const usage = node.reused_from ? node.usage : agent?.usage ?? node.usage;
  return <>
    {agent && <dl className="workflow-facts">
      <dt>{t.model}</dt><dd>{agent.model ?? t.unknown}</dd>
      <dt>{t.agentRun}</dt><dd>{agent.id}</dd>
      <dt>{t.duration}</dt><dd>{workflowDuration(agent.started_at_ms, agent.finished_at_ms, t)}</dd>
      <dt>{t.started}</dt><dd><Timestamp value={agent.started_at_ms} language={language} fallback={t.notStarted} /></dd>
      <dt>{t.finished}</dt><dd><Timestamp value={agent.finished_at_ms} language={language} fallback={["queued", "running", "waiting_approval", "waiting_answer"].includes(agent.status) ? t.active : t.unknown} /></dd>
    </dl>}
    <dl className="workflow-facts" aria-label={t.usage}>
      <dt>{t.tokens}</dt><dd>{usage.total_tokens ?? t.unknown}</dd>
      <dt>{t.inputTokens}</dt><dd>{usage.input_tokens ?? t.unknown}</dd>
      <dt>{t.outputTokens}</dt><dd>{usage.output_tokens ?? t.unknown}</dd>
      <dt>{t.cacheTokens}</dt><dd>{usage.cache_read_tokens ?? t.unknown}</dd>
      <dt>{t.accounted}</dt><dd>{usage.accounted_tokens}</dd>
    </dl>
    {usage.unknown_usage_calls > 0 && <p className="workflow-note">{t.unknownCalls}: {usage.unknown_usage_calls}</p>}
    {usage.uncertain_execution && <p className="workflow-note">{t.uncertain}</p>}
    {node.reused_from && <dl className="workflow-facts"><dt>{t.reused}</dt><dd>{node.reused_from.run_id}</dd><dt>{t.instance}</dt><dd>{node.reused_from.node_instance_id}</dd></dl>}
  </>;
}

type ScopeProps = { definitions: WorkflowNode[]; prefix: string; run: WorkflowRun; language: Language; messages: Messages };
function NodeRow({ definition, prefix, run, language, messages }: Omit<ScopeProps, "definitions"> & { definition: WorkflowNode }) {
  const t = workflowText(language);
  const path = prefix ? `${prefix}/${definition.id}` : definition.id;
  const node = run.nodes.find((candidate) => candidate.path === path);
  const descendants = run.nodes.filter((candidate) => candidate.path.startsWith(`${path}/`));
  const iterations = [...new Set(descendants.map((candidate) => candidate.path.slice(path.length + 1).split("/")[0]))]
    .filter((index) => /^\d+$/.test(index)).sort((a, b) => Number(a) - Number(b));
  return <li data-node-path={path}>
    <details className="workflow-node" open={definition.kind === "agent" ? undefined : false}>
      <summary><span className="workflow-node-name">{definition.id}</span><span className="workflow-note">{t.kinds[definition.kind]}</span><WorkflowStatus status={node?.status ?? "pending"} t={t} /></summary>
      <div className="workflow-node-body">
        {definition.depends_on.length > 0 && <p className="workflow-note">{t.dependsOn}: {definition.depends_on.join(", ")}</p>}
        {node && <>
          <NodeUsage node={node} t={t} language={language} />
          <dl className="workflow-facts"><dt>{t.instance}</dt><dd>{node.id}</dd><dt>{t.attempt}</dt><dd>{node.attempt_id}</dd></dl>
          {!node.agent_run && <dl className="workflow-facts"><dt>{t.duration}</dt><dd>{workflowDuration(node.started_at_ms, node.finished_at_ms, t)}</dd></dl>}
          {node.error && <p className="workflow-error" role="alert">{t.error}: {node.error}</p>}
          <Report label={t.report} content={node.report ?? node.agent_run?.report ?? null} messages={messages} />
          <ValueDetails label={t.output} value={node.output} />
        </>}
        {(definition.kind === "foreach" || definition.kind === "repeat") && <>
          {iterations.length === 0 && <p className="workflow-note">{t.noInstances}</p>}
          {iterations.map((index) => <details key={index} className="workflow-iteration"><summary>{definition.kind === "foreach" ? t.item : t.iteration} {Number(index) + 1}</summary>
            <WorkflowScopeView definitions={definition.body.nodes} prefix={`${path}/${index}`} run={run} language={language} messages={messages} />
          </details>)}
        </>}
        {definition.kind === "switch" && (["then", "else"] as const).map((branch) => descendants.some((child) => child.path.startsWith(`${path}/${branch}/`)) &&
          <details key={branch} className="workflow-iteration" open><summary>{t[branch]}</summary><WorkflowScopeView definitions={definition[`${branch}_scope`].nodes} prefix={`${path}/${branch}`} run={run} language={language} messages={messages} /></details>)}
      </div>
    </details>
  </li>;
}
function WorkflowScopeView(props: ScopeProps) {
  const t = workflowText(props.language);
  const phases = props.run.spec.phases;
  const groups = phases.map((phase) => ({ ...phase, nodes: props.definitions.filter((node) => node.phase === phase.id) }));
  const ungrouped = props.definitions.filter((node) => !phases.some((phase) => phase.id === node.phase));
  if (ungrouped.length) groups.push({ id: "__ungrouped", name: t.ungrouped, nodes: ungrouped });
  return <div className="workflow-phases">{groups.filter((group) => group.nodes.length > 0).map((group) => <details className="workflow-phase" key={group.id} open>
    <summary>{group.name}</summary>
    <ul>{group.nodes.map((definition) => <NodeRow key={definition.id} definition={definition} prefix={props.prefix} run={props.run} language={props.language} messages={props.messages} />)}</ul>
  </details>)}</div>;
}
export function WorkflowRunView({ run, language, messages }: { run: WorkflowRun; language: Language; messages: Messages }) {
  const t = workflowText(language);
  return <div className="workflow-run-view">
    <dl className="workflow-facts"><dt>{t.runId}</dt><dd>{run.id}</dd>
      {run.source_run_id && <><dt>{t.source}</dt><dd>{run.source_run_id}</dd></>}
      <dt>{t.created}</dt><dd><Timestamp value={run.created_at_ms} language={language} fallback={t.unknown} /></dd>
      <dt>{t.duration}</dt><dd>{workflowDuration(run.created_at_ms, run.finished_at_ms, t)}</dd>
    </dl>
    {run.error && <p className="workflow-error" role="alert">{t.error}: {run.error}</p>}
    <Report label={t.report} content={run.report} messages={messages} />
    <ValueDetails label={t.output} value={run.output} />
    <ValueDetails label={t.input} value={run.input} />
    {run.report !== null && <p className="workflow-note">{run.acknowledged ? t.acknowledged : t.unacknowledged}</p>}
    <p className="workflow-note">{t.phasesHint}</p>
    <WorkflowScopeView definitions={run.spec.nodes} prefix="" run={run} language={language} messages={messages} />
  </div>;
}
