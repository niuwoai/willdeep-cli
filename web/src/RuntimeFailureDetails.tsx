import { useRef, useState } from "react";
import type { Messages } from "./i18n";

type FailedTool = {
  sequence: number;
  name: string;
  arguments: string | null;
  output: string | null;
};

type TaskDiagnostics = {
  failure_domain: string | null;
  exit_code: number | null;
  failure: string | null;
  failed_tools: FailedTool[];
};

const failureDomains: Record<string, keyof Messages> = {
  provider: "failureProvider",
  policy: "failurePolicy",
  tool: "failureTool",
  harness: "failureHarness",
  internal: "failureInternal",
  unknown: "failureInternal",
};

export function RuntimeFailureDetails({ sessionId, taskId, messages }: {
  sessionId: string;
  taskId: string;
  messages: Messages;
}) {
  const [diagnostics, setDiagnostics] = useState<TaskDiagnostics | null>(null);
  const [loading, setLoading] = useState(false);
  const [failed, setFailed] = useState(false);
  const requested = useRef(false);

  async function load() {
    if (requested.current || loading) return;
    requested.current = true;
    setFailed(false);
    setLoading(true);
    try {
      const response = await fetch(`/api/sessions/${encodeURIComponent(sessionId)}/tasks/${encodeURIComponent(taskId)}/diagnostics`);
      if (!response.ok) throw new Error(messages.diagnosticLoadFailed);
      setDiagnostics(await response.json() as TaskDiagnostics);
    } catch {
      requested.current = false;
      setFailed(true);
    } finally {
      setLoading(false);
    }
  }

  const hasDetails = diagnostics && (diagnostics.failure || diagnostics.failed_tools.length || diagnostics.exit_code !== null);
  const domainKey = diagnostics?.failure_domain ? failureDomains[diagnostics.failure_domain] : undefined;

  return <details
    className="runtime-failure-details"
    onToggle={(event) => { if (event.currentTarget.open) void load(); }}
  >
    <summary>{messages.diagnosticDetails}</summary>
    {loading && <p role="status">{messages.diagnosticLoading}</p>}
    {failed && <p role="alert">{messages.diagnosticLoadFailed}</p>}
    {diagnostics && !hasDetails && <p>{messages.diagnosticEmpty}</p>}
    {diagnostics && hasDetails && <div className="runtime-failure-content">
      {(domainKey || diagnostics.exit_code !== null) && <dl>
        {domainKey && <><dt>{messages.failureDomain}</dt><dd>{messages[domainKey]}</dd></>}
        {diagnostics.exit_code !== null && <><dt>{messages.exitCode}</dt><dd>{diagnostics.exit_code}</dd></>}
      </dl>}
      {diagnostics.failure && <section>
        <h3>{messages.diagnosticFailure}</h3>
        <pre>{diagnostics.failure}</pre>
      </section>}
      {diagnostics.failed_tools.map((tool) => <section key={`${tool.sequence}-${tool.name}`}>
        <h3>{messages.diagnosticFailedTools}: {tool.name}</h3>
        {tool.arguments && <><h4>{messages.diagnosticArguments}</h4><pre>{tool.arguments}</pre></>}
        {tool.output && <><h4>{messages.diagnosticOutput}</h4><pre>{tool.output}</pre></>}
      </section>)}
    </div>}
  </details>;
}
