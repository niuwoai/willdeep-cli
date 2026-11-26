import { useId } from "react";
import { createRequestId } from "./requestId";
import { CANONICAL_REASONING_LEVELS } from "./reasoningLevels";
import type { reasoningText } from "./reasoningMessages";
import type { ReasoningDraft, ReasoningEntry as Entry } from "./reasoningDraft";

type Labels = ReturnType<typeof reasoningText>;

function LevelInput({ label, hint, value, onChange }: {
  label: string; hint: string; value: string; onChange: (value: string) => void;
}) {
  const id = useId();
  return <div className="configuration-reasoning-level">
    <label htmlFor={`${id}-level`}>{label}</label>
    <input id={`${id}-level`} type="text" value={value} aria-describedby={`${id}-hint`} onChange={event => onChange(event.target.value)} />
    <div id={`${id}-hint`} className="sub">{hint}</div>
    <div className="presets" role="group" aria-label={label}>
      {CANONICAL_REASONING_LEVELS.map(level => <button key={level} type="button" className="btn btn-sm" aria-controls={`${id}-level`} onClick={() => onChange(level)}>{level}</button>)}
    </div>
  </div>;
}

export function ReasoningFields({ draft, labels: t, disabled, onChange }: {
  draft: ReasoningDraft; labels: Labels; disabled?: boolean; onChange: (draft: ReasoningDraft) => void;
}) {
  function changeEntry(id: string, patch: Partial<Entry>) {
    onChange({ ...draft, levels: draft.levels.map(entry => entry.id === id ? { ...entry, ...patch } : entry) });
  }
  return <fieldset className="configuration-reasoning-fields" disabled={disabled}>
    <div className="configuration-reasoning-row">
      <LevelInput label={t.defaultLevel} hint={t.defaultLevelHint} value={draft.default_level} onChange={value => onChange({ ...draft, default_level: value })} />
      <label><span>{t.parameter}</span><input type="text" value={draft.parameter} placeholder="reasoning_effort" onChange={event => onChange({ ...draft, parameter: event.target.value })} /></label>
    </div>
    {draft.levels.map(entry => <div key={entry.id} className="configuration-reasoning-row">
      <LevelInput label={t.levelName} hint={t.levelNameHint} value={entry.level} onChange={level => changeEntry(entry.id, { level })} />
      <label><span>{t.valueType}</span><select value={entry.kind} onChange={event => {
        const kind = event.target.value as Entry["kind"];
        changeEntry(entry.id, { kind, value: kind === "boolean" ? "false" : kind === "number" ? "0" : "" });
      }}><option value="string">{t.string}</option><option value="number">{t.number}</option><option value="boolean">{t.boolean}</option></select></label>
      <label><span>{t.value}</span>{entry.kind === "boolean" ? <select value={entry.value} onChange={event => changeEntry(entry.id, { value: event.target.value })}>
        <option value="false">{t.falseValue}</option><option value="true">{t.trueValue}</option>
      </select> : <input type={entry.kind === "number" ? "number" : "text"} step={entry.kind === "number" ? "any" : undefined} value={entry.value} onChange={event => changeEntry(entry.id, { value: event.target.value })} />}</label>
      <button type="button" className="btn btn-sm configuration-reasoning-remove" onClick={() => onChange({ ...draft, levels: draft.levels.filter(row => row.id !== entry.id) })}>{t.remove}</button>
    </div>)}
    <button type="button" className="btn btn-sm" onClick={() => onChange({ ...draft, levels: [...draft.levels, { id: createRequestId(), level: "", kind: "string", value: "" }] })}>{t.addLevel}</button>
  </fieldset>;
}
