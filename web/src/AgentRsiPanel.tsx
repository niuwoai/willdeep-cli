import { useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import type { Language } from "./i18n";
import { agentLibraryCall, nextAgentVersion } from "./agentLibraryModel";
import { agentText } from "./agentLibraryMessages";

type Definition = { definition_id: string; version: string; instructions: string; [key: string]: unknown };
export type RsiPackage = { definition: Definition; sha256: string; signature: unknown };
type Experiment = {experiment_id: string; definition_id: string; candidate_revision: string};
function record(value: unknown): value is Record<string, unknown> {return typeof value === "object" && value !== null;}
export function AgentRsiPanel({ language, baseline, onClose, accountScope = "local" }: {language: Language; baseline: RsiPackage | null; onClose: () => void; accountScope?: string}) {
  const t = (key: Parameters<typeof agentText>[0]) => agentText(key, language);
  const client = useQueryClient();
  const experiments = useQuery({queryKey: ["agent-rsi", accountScope], queryFn: () => agentLibraryCall<{items: Experiment[]}>("rsi.list")});
  const [instructions, setInstructions] = useState(baseline?.definition.instructions ?? "");
  const [result, setResult] = useState<unknown>(null);
  const mutation = useMutation({mutationFn: ({operation, params}: {operation: string; params: unknown}) => agentLibraryCall(`rsi.${operation}`, params), onSuccess: async value => {setResult(value); await client.invalidateQueries({queryKey: ["agent-rsi"]});}});
  const act = (operation: string, params: unknown) => mutation.mutate({operation, params});
  const candidate = baseline ? {...baseline.definition, version: nextAgentVersion(baseline.definition.version), instructions, forked_from: baseline.sha256} : null;
  const official = Boolean(baseline?.definition.definition_id.startsWith("official/") || baseline?.signature);
  const create = (operation: string) => {if (baseline && candidate && !official) act(operation, {id: `exp-${crypto.randomUUID()}`, baseline_package: baseline, candidate_definition: candidate});};
  const resultSummary = () => {
    if (!record(result)) return t("done");
    if (typeof result.uploaded_evidence === "number") return t("rsiUploadSummary").replace("{count}", String(result.uploaded_evidence));
    if (typeof result.attempts === "number") {
      const verified = Array.isArray(result.evidence) ? result.evidence.filter(row => record(row) && row.status === "passed").length : 0;
      const summary = t("rsiRunSummary").replace("{attempts}", String(result.attempts)).replace("{verified}", String(verified));
      return result.verdict === "inconclusive" ? `${t("rsiIncomplete")} ${summary}` : summary;
    }
    return "experiment" in result ? t("rsiPrepared") : "candidate" in result ? t("rsiCandidateSaved") : t("done");
  };
  return <section className="agent-library-editor" aria-label={t("rsiTitle")}>
    <h2>{t("rsiTitle")}</h2><p>{t("rsiHelp")}</p>
    {baseline && <><details><summary>{t("rsiDetails")}</summary><code>{baseline.definition.definition_id} · {baseline.sha256}</code></details>
      {official && <p role="status">{t("rsiForkRequired")}</p>}
      <label>{t("rsiInstruction")}<textarea aria-label={t("rsiInstruction")} value={instructions} disabled={official} onChange={event => setInstructions(event.target.value)} /></label>
      <button type="button" disabled={official || mutation.isPending || instructions === baseline.definition.instructions} onClick={() => create("candidate")}>{t("rsiCandidate")}</button>
      <button type="button" disabled={official || mutation.isPending || instructions === baseline.definition.instructions} onClick={() => create("prepare")}>{t("rsiPrepare")}</button></>}
    {(mutation.error || experiments.error) && <p role="alert">{t("failed")}: {(mutation.error || experiments.error)?.message}</p>}
    {mutation.isPending && <p role="status">{t("busy")}</p>}
    {experiments.data?.items.map((item, index) => <article key={item.experiment_id}><h3>{t("rsiExperimentNumber").replace("{number}", String(index + 1))}</h3><details><summary>{t("rsiDetails")}</summary><code>{item.definition_id} · {item.experiment_id} · {item.candidate_revision}</code></details>
      <button type="button" disabled={mutation.isPending} onClick={() => act("run", {id: item.experiment_id, agent_revision: item.candidate_revision, resume: true})}>{t("rsiRun")}</button>
      <button type="button" disabled={mutation.isPending} onClick={() => act("report", {id: item.experiment_id})}>{t("rsiReport")}</button>
      <button type="button" disabled={mutation.isPending} onClick={() => {const grant = window.prompt(t("rsiGrant")); if (grant) act("upload", {id: item.experiment_id, grant_id: grant});}}>{t("rsiUpload")}</button>
    </article>)}
    {result !== null && <><p role="status">{resultSummary()}</p><p>{t("rsiNoActivation")}</p><details><summary>{t("rsiDetails")}</summary><pre>{JSON.stringify(result, null, 2)}</pre></details></>}
    <button type="button" onClick={onClose}>{t("cancel")}</button>
  </section>;
}
