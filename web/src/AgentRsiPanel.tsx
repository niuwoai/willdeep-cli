import { useEffect, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { FileText, Play, Save, Info, MoreHorizontal, Users } from "lucide-react";
import type { Language } from "./i18n";
import { agentLibraryCall, nextAgentVersion } from "./agentLibraryModel";
import { agentText } from "./agentLibraryMessages";
import { readRsiNote, writeRsiNote } from "./rsiPresentation";

type Definition = { definition_id: string; version: string; instructions: string; name?: Record<string, string>; purpose?: string; [key: string]: unknown };
export type RsiPackage = { definition: Definition; sha256: string; signature: unknown };
type Experiment = {experiment_id: string; definition_id: string; baseline_revision: string; candidate_revision: string};
function record(value: unknown): value is Record<string, unknown> {return typeof value === "object" && value !== null;}
type Props = {language: Language; baseline: RsiPackage | null; onClose: () => void; accountScope?: string; onDirty?: (dirty: boolean) => void; definitionNames?: Record<string,string>};
export function AgentRsiPanel(props: Props) {return <AgentRsiWorkbench key={`${props.accountScope ?? "local"}/${props.baseline?.sha256 ?? "none"}`} {...props} />;}
function AgentRsiWorkbench({ language, baseline, onClose, accountScope = "local", onDirty, definitionNames = {} }: Props) {
  const t = (key: Parameters<typeof agentText>[0]) => agentText(key, language);
  const client = useQueryClient();
  const experiments = useQuery({queryKey: ["agent-rsi", accountScope], queryFn: () => agentLibraryCall<{items: Experiment[]}>("rsi.list")});
  const [instructions, setInstructions] = useState(baseline?.definition.instructions ?? "");
  const [savedInstructions, setSavedInstructions] = useState(baseline?.definition.instructions ?? "");
  const dirty = instructions !== savedInstructions;
  useEffect(() => {onDirty?.(dirty); return () => onDirty?.(false);}, [dirty, onDirty]);
  useEffect(() => {if (!dirty) return; const protect = (event: BeforeUnloadEvent) => {event.preventDefault(); event.returnValue = "";}; window.addEventListener("beforeunload", protect); return () => window.removeEventListener("beforeunload", protect);}, [dirty]);
  const [note, setNote] = useState(() => readRsiNote(accountScope, `draft/${baseline?.sha256 ?? "none"}`));
  const [noteStored, setNoteStored] = useState(true);
  const [result, setResult] = useState<unknown>(null);
  const mutation = useMutation({mutationFn: ({operation, params}: {operation: string; params: unknown}) => agentLibraryCall(`rsi.${operation}`, params), onSuccess: async (value, variables) => {setResult(value); if (["candidate", "prepare"].includes(variables.operation)) setSavedInstructions(instructions); if (record(value) && record(value.experiment) && typeof value.experiment.experiment_id === "string" && note) setNoteStored(writeRsiNote(accountScope, value.experiment.experiment_id, note)); await client.invalidateQueries({queryKey: ["agent-rsi", accountScope]});}});
  const act = (operation: string, params: unknown) => mutation.mutate({operation, params});
  const candidate = baseline ? {...baseline.definition, version: nextAgentVersion(baseline.definition.version), instructions, forked_from: baseline.sha256} : null;
  const official = Boolean(baseline?.definition.definition_id.startsWith("official/") || baseline?.signature);
  const create = (operation: string) => {if (baseline && candidate && !official) act(operation, {id: `exp-${crypto.randomUUID()}`, baseline_package: baseline, candidate_definition: candidate});};
  const name = baseline?.definition.name?.[language === "zh-CN" ? "zh-Hans" : language] ?? baseline?.definition.name?.en ?? baseline?.definition.definition_id;
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
  const step = result === null ? (baseline ? 1 : 0) : record(result) && typeof result.attempts === "number" ? 3 : 2;
  return <section className="rsi-workbench" aria-label={t("rsiTitle")}>
    <ol className="rsi-steps" aria-label={t("offlineMode")}><li className="rsi-mode">{t("offlineMode")}</li>{(["stepBaseline", "stepCandidate", "stepVerify", "stepResults"] as const).map((key, index) => <li key={key} className={index === step ? "current" : index < step ? "complete" : ""} aria-current={index === step ? "step" : undefined}><span>{index + 1}</span>{t(key)}</li>)}</ol>
    {baseline && <>
      <label className="rsi-note">{t("experimentNote")}<input maxLength={160} value={note} onChange={event => {setNote(event.target.value); setNoteStored(writeRsiNote(accountScope, `draft/${baseline.sha256}`, event.target.value));}} placeholder={`${name} · ${candidate?.version}`} /></label><p className="rsi-note-help">{t(noteStored ? "experimentNoteHelp" : "noteUnavailable")}</p>
      <div className="rsi-comparison">
        <article className="rsi-revision baseline-revision"><div className="rsi-revision-heading"><h2>{t("baseline")} <span className="agent-tag">{t("selectedRevision")}</span></h2><span>{baseline.definition.version}</span></div>
          <div className="rsi-agent-identity"><FileText size={24} /><div><h3>{name}</h3><p>{baseline.definition.purpose}</p></div></div>
          <h3>{t("baselineInstructions")}</h3><div className="rsi-readonly-instructions">{baseline.definition.instructions}</div>
          <details className="rsi-baseline-details"><summary>{t("baselineDetails")}</summary><code>{baseline.definition.definition_id} · {baseline.sha256}</code><pre>{JSON.stringify(baseline.definition, null, 2)}</pre></details>
        </article>
        <article className="rsi-revision candidate-revision"><div className="rsi-revision-heading"><h2>{t("candidate")}</h2><span>{candidate?.version}</span></div>
          <div className="rsi-agent-identity"><FileText size={24} /><div><h3>{name}</h3><p>{baseline.definition.purpose}</p></div></div>
          {official && <p role="status">{t("rsiForkRequired")}</p>}
          <label className="rsi-candidate-label">{t("rsiInstruction")}<textarea aria-label={t("rsiInstruction")} value={instructions} disabled={official || mutation.isPending} onChange={event => setInstructions(event.target.value)} /></label>
          <div className="agent-library-actions"><button className="agent-primary" type="button" disabled={official || mutation.isPending || instructions === baseline.definition.instructions} onClick={() => create("candidate")}><Save size={18} />{t("rsiCandidate")}</button><button type="button" disabled={official || mutation.isPending || instructions === baseline.definition.instructions} onClick={() => create("prepare")}><Play size={18} />{t("rsiPrepare")}</button></div>
          <p className="rsi-reassurance"><Info size={16} />{t("candidateSeparate")}</p>
        </article>
      </div>
    </>}
    {!baseline && <div className="agent-empty-state"><Users size={30} /><h2>{t("stepBaseline")}</h2><p>{t("chooseBaselineHelp")}</p><button type="button" onClick={onClose}>{t("openLibrary")}</button></div>}
    {(mutation.error || experiments.error) && <p role="alert">{t("failed")}: {(mutation.error || experiments.error)?.message}</p>}
    {mutation.isPending && <p role="status">{t("busy")}</p>}
    {result !== null && <aside className="rsi-result"><p role="status">{resultSummary()}</p><p>{t("rsiNoActivation")}</p><details><summary>{t("rsiDetails")}</summary><pre>{JSON.stringify(result, null, 2)}</pre></details></aside>}
    <section className="rsi-recent"><h2>{t("recentExperiments")}</h2><p>{t("registeredHelp")}</p>
      {!experiments.data?.items.length && !experiments.isLoading && <p className="agent-empty-inline">{t("noExperiments")}</p>}
      <div className="rsi-experiment-list">{experiments.data?.items.map((item, index) => <article key={item.experiment_id}>
        <div><h3>{readRsiNote(accountScope, item.experiment_id) || definitionNames[item.definition_id] || t("rsiExperimentNumber").replace("{number}", String(index + 1))}</h3><span className="agent-tag">{t("registered")}</span></div>
        <button type="button" disabled={mutation.isPending} onClick={() => act("run", {id: item.experiment_id, agent_revision: item.candidate_revision, resume: true})}><Play size={16} />{t("rsiRun")}</button>
        <details className="agent-overflow"><summary aria-label={t("moreActions")}><MoreHorizontal size={20} /></summary><div><button type="button" disabled={mutation.isPending} onClick={() => act("report", {id: item.experiment_id})}>{t("rsiReport")}</button><button type="button" disabled={mutation.isPending} onClick={() => {const grant = window.prompt(t("rsiGrant")); if (grant) act("upload", {id: item.experiment_id, grant_id: grant});}}>{t("rsiUpload")}</button><details><summary>{t("rsiDetails")}</summary><code>{item.definition_id} · {item.experiment_id}</code><p>{t("baselineRevision")}: {item.baseline_revision}</p><p>{t("candidateRevision")}: {item.candidate_revision}</p></details></div></details>
      </article>)}</div>
    </section>
    <details className="rsi-boundary"><summary>{t("offlineMode")}</summary><p>{t("rsiHelp")}</p></details>
  </section>;
}
