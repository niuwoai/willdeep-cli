import { useState } from "react";
import type { Messages, Language } from "./i18n";
import type { RuntimeGate } from "./RuntimeSidebar";
import { queueText } from "./queueMessages";

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
  async function act(action: () => Promise<void>) {
    setBusy(true); setFailed(false);
    try { await action(); } catch { setFailed(true); } finally { setBusy(false); }
  }
  return <article className="queued-messages">
    <strong>{gate.kind === "approval" ? t.waitingApproval : t.waitingAnswer}</strong>
    <p>{gate.kind === "approval" ? gate.description : gate.question}</p>
    <p>{queueText("originalRetained", language)}</p>
    <details open={!gate.parked}><summary>{t.details}</summary>
      {gate.kind === "approval" ? <div className="gate-actions">
        <button type="button" className="gate-action primary" disabled={busy} onClick={() => void act(() => onApproval(gate.id, "allow_once"))}>{t.allowOnce}</button>
        <button type="button" className="gate-action secondary" disabled={busy} onClick={() => void act(() => onApproval(gate.id, "deny"))}>{t.deny}</button>
        {gate.always_allow_available && <button type="button" className="gate-action tertiary" disabled={busy} onClick={() => void act(() => onApproval(gate.id, "always_allow"))}>{t.alwaysAllow}</button>}
      </div> : <div>
        {gate.options.map((option) => gate.multi_select
          ? <label key={option}><input type="checkbox" checked={selected.includes(option)} onChange={(event) => setSelected((values) => event.target.checked ? [...values, option] : values.filter((value) => value !== option))} />{option}</label>
          : <button key={option} disabled={busy} onClick={() => void act(() => onAnswer(gate.id, option))}>{option}</button>)}
        {gate.multi_select && <button disabled={busy || !selected.length} onClick={() => void act(() => onAnswer(gate.id, selected.join(", ")))}>{t.submitAnswer}</button>}
        <input aria-label={t.otherAnswer} value={answer} onChange={(event) => setAnswer(event.target.value)} />
        <button disabled={busy || !answer.trim()} onClick={() => void act(() => onAnswer(gate.id, answer.trim()))}>{t.submitAnswer}</button>
      </div>}
    </details>
    {failed && <p role="alert">{t.runtimeActionFailed}</p>}
  </article>;
}
