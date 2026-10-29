import { useRef, useState } from "react";
import { ChevronRight } from "lucide-react";
import type { Messages, Language } from "./i18n";
import type { RuntimeGate } from "./RuntimeSidebar";
import { queueText } from "./queueMessages";
import "./humanGates.css";

type Props = {
  gates: RuntimeGate[]; messages: Messages; language: Language;
  onApproval: (id: string, decision: "allow_once" | "deny" | "always_allow") => Promise<void>;
  onAnswer: (id: string, answer: string | null) => Promise<void>;
};

export function RuntimeHumanGates(props: Props) {
  return <section>{props.gates.map((gate) => <HumanGate key={gate.id} {...props} gate={gate} />)}</section>;
}

function HumanGate({ gate, messages: t, language, onApproval, onAnswer }: Props & { gate: RuntimeGate }) {
  const [answer, setAnswer] = useState("");
  const [selected, setSelected] = useState<string[]>([]);
  const [busy, setBusy] = useState(false);
  const [failed, setFailed] = useState(false);
  const [submitted, setSubmitted] = useState(false);
  const submitting = useRef(false);
  const disabled = busy || submitted;
  async function act(action: () => Promise<void>) {
    if (submitting.current) return;
    submitting.current = true;
    setBusy(true); setFailed(false);
    try { await action(); setSubmitted(true); }
    catch { setFailed(true); submitting.current = false; }
    finally { setBusy(false); }
  }
  return <article id={`runtime-gate-${gate.id}`} tabIndex={-1} className="queued-messages human-gate" aria-label={gate.kind === "approval" ? gate.description : gate.question} aria-busy={busy}>
    <strong>{gate.kind === "approval" ? t.waitingApproval : t.waitingAnswer}</strong>
    <p>{gate.kind === "approval" ? gate.description : gate.question}</p>
    {gate.parked && <p className="human-gate-note">{queueText("originalRetained", language)}</p>}
      {gate.kind === "approval" ? <div className="gate-actions">
        <button type="button" className="gate-action primary" disabled={disabled} onClick={() => void act(() => onApproval(gate.id, "allow_once"))}>{t.allowOnce}</button>
        <button type="button" className="gate-action secondary" disabled={disabled} onClick={() => void act(() => onApproval(gate.id, "deny"))}>{t.deny}</button>
        {gate.always_allow_available && <button type="button" className="gate-action tertiary" disabled={disabled} onClick={() => void act(() => onApproval(gate.id, "always_allow"))}>{t.alwaysAllow}</button>}
      </div> : <fieldset className="human-gate-fields" disabled={disabled}>
        <div className="human-gate-options" role="group" aria-label={gate.question}>
        {gate.options.map((option) => gate.multi_select
          ? <label className="human-gate-option multiple" key={option}><input type="checkbox" checked={selected.includes(option)} onChange={(event) => setSelected((values) => event.target.checked ? [...values, option] : values.filter((value) => value !== option))} /><span>{option}</span></label>
          : <button type="button" className="human-gate-option" key={option} onClick={() => void act(() => onAnswer(gate.id, option))}><span>{option}</span><ChevronRight size={16} aria-hidden="true" /></button>)}
        </div>
        {gate.multi_select && <button type="button" className="gate-action primary" disabled={!selected.length} onClick={() => void act(() => onAnswer(gate.id, selected.join(", ")))}>{t.submitAnswer}</button>}
        <form className="human-gate-answer" onSubmit={(event) => { event.preventDefault(); if (answer.trim()) void act(() => onAnswer(gate.id, answer.trim())); }}>
          <label htmlFor={`gate-answer-${gate.id}`}>{t.otherAnswer}</label>
          <div><input id={`gate-answer-${gate.id}`} placeholder={t.otherAnswer} value={answer} onChange={(event) => setAnswer(event.target.value)} />
          <button type="submit" className="gate-action secondary" disabled={!answer.trim()}>{t.submitAnswer}</button></div>
        </form>
      </fieldset>}
    {submitted && <p role="status">{t.gateSubmitted}</p>}
    {failed && <p role="alert">{t.runtimeActionFailed}</p>}
  </article>;
}
