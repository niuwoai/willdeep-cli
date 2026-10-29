import type { Messages } from "./i18n";
import { Markdown } from "./Markdown";
import { markdownCopy } from "./markdownCopy";
import "./conversation.css";
import { Ban, CheckCircle2, Circle, Clock3, LoaderCircle, MinusCircle, Workflow, XCircle, type LucideIcon } from "lucide-react";

type StepStatus = "pending" | "in_progress" | "done" | "skipped" | "failed" | "blocked";
export type Plan = { summary: string; steps: Array<{ id: string; text: string; status: StepStatus; detail?: string | null; depends_on?: string[] }> };

const STATUS_ICONS: Record<StepStatus, LucideIcon> = {
  pending: Circle, in_progress: LoaderCircle, done: CheckCircle2,
  skipped: MinusCircle, failed: XCircle, blocked: Ban,
};
export type ConversationItem = {
  role: "user" | "assistant" | "system" | "plan";
  content: string;
  attachment_count: number;
  attachment_reference?: string;
  plan?: Plan;
  details?: string[];
  /// 这条助手消息发起的工具调用：名字、脱敏摘要、有无结果。原始参数不下发。
  tools?: { name: string; detail?: string; completed: boolean }[];
};

export function ConversationCard({ plan, details = [], messages: t }: { plan?: Plan; details?: string[]; messages: Messages }) {
  const statuses: Record<StepStatus, string> = {
    pending: t.planPending, in_progress: t.toolRunning, done: t.toolDone,
    skipped: t.planSkipped, failed: t.toolFailed, blocked: t.blocked,
  };
  const finished = plan?.steps.filter((step) => step.status === "done" || step.status === "skipped").length ?? 0;
  const running = plan?.steps.filter((step) => step.status === "in_progress").length ?? 0;
  if (!plan) return <details className="host-event" aria-label={t.hostActivity}>
    <summary><Workflow size={14} /><span>{t.hostActivity}</span><small>{t.conversationOriginal}</small></summary>
    {details.map((content) => <pre key={content}>{content}</pre>)}
  </details>;
  return <section className="conversation-card" aria-label={plan ? t.planTitle : t.hostActivity}>
    <div className="conversation-card-heading">
      <strong>{plan ? t.planTitle : t.hostActivity}</strong>
      {plan && <span>{t.planProgress.replace("{finished}", String(finished)).replace("{total}", String(plan.steps.length))}</span>}
      {running > 0 && <span role="status">{t.planRunning.replace("{count}", String(running))}</span>}
    </div>
    {plan?.summary && <Markdown content={plan.summary} copy={markdownCopy(t)} />}
    {plan && <ol className="conversation-plan-steps">
      {plan.steps.map((step) => {
        const waiting = (step.depends_on ?? []).filter((id) => !plan.steps.some((other) => other.id === id && other.status === "done"));
        const isWaiting = step.status === "pending" && waiting.length > 0;
        const dependencies = waiting.map((id) => plan.steps.find((other) => other.id === id)?.text ?? id).join(t.planDependencySeparator);
        const StatusIcon = isWaiting ? Clock3 : STATUS_ICONS[step.status];
        return <li key={step.id} data-status={isWaiting ? "waiting" : step.status}>
          <span className="conversation-plan-status">
            <StatusIcon size={13} aria-hidden="true" className={!isWaiting && step.status === "in_progress" ? "plan-spin" : undefined} />
            {isWaiting ? t.planWaiting : statuses[step.status]}
          </span>
          <div><span>{step.text}</span>{step.detail && <p>{step.detail}</p>}
            {isWaiting && <p>{t.planWaitingFor.replace("{steps}", dependencies)}</p>}
          </div>
        </li>;
      })}
    </ol>}
    {details.length > 0 && <details className="conversation-card-details">
      <summary>{t.conversationOriginal}</summary>
      {details.map((content) => <pre key={content}>{content}</pre>)}
    </details>}
  </section>;
}
