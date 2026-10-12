import type { Language } from "./i18n";
import { scheduleText } from "./scheduleMessages";
import type { ScheduleDraft, ScheduleSpec } from "./scheduleTypes";

export function ScheduleTiming({ draft, timezoneOptions, onceTime, onOnceTime, onChange, language, disabled }: {
  draft: ScheduleDraft; timezoneOptions: string[]; onceTime: string; onOnceTime: (value: string) => void;
  onChange: (draft: ScheduleDraft) => void; language: Language; disabled: boolean;
}) {
  const s = (key: Parameters<typeof scheduleText>[0]) => scheduleText(key, language);
  const spec = draft.schedule;
  const time = "hour" in spec ? `${String(spec.hour).padStart(2, "0")}:${String(spec.minute).padStart(2, "0")}` : "09:00";
  const changeKind = (kind: ScheduleSpec["kind"]) => {
    const schedule: ScheduleSpec = kind === "interval" ? { kind, minutes: 30 } : kind === "onceAt" ? { kind, at: 0 }
      : kind === "weeklyAt" ? { kind, weekday: 2, hour: 9, minute: 0 } : { kind, hour: 9, minute: 0 };
    onChange({ ...draft, schedule });
  };
  const changeTime = (value: string) => {
    const [hour, minute] = value.split(":").map(Number);
    if ("hour" in spec) onChange({ ...draft, schedule: { ...spec, hour, minute } });
  };
  const zones = Array.from(new Set([draft.timezone, ...timezoneOptions])).filter(Boolean);
  return <fieldset disabled={disabled} className="schedule-fields">
    <label>{s("time")}<select value={spec.kind} onChange={(event) => changeKind(event.target.value as ScheduleSpec["kind"])}>
      {(["interval", "dailyAt", "weekdaysAt", "weeklyAt", "onceAt"] as const).map((kind) => <option key={kind} value={kind}>{s(kind)}</option>)}
    </select></label>
    <label>{s("timezone")}<input list="schedule-timezones" value={draft.timezone} onChange={(event) => onChange({ ...draft, timezone: event.target.value })} required />
      <datalist id="schedule-timezones">{zones.map((zone) => <option key={zone} value={zone} />)}</datalist>
    </label>
    {spec.kind === "interval" && <label>{s("minutes")}<input type="number" min="1" value={spec.minutes} onChange={(event) => onChange({ ...draft, schedule: { ...spec, minutes: Number(event.target.value) } })} required /></label>}
    {"hour" in spec && <label>{s("time")}<input type="time" value={time} onChange={(event) => changeTime(event.target.value)} required /></label>}
    {spec.kind === "weeklyAt" && <label>{s("weekday")}<select value={spec.weekday} onChange={(event) => onChange({ ...draft, schedule: { ...spec, weekday: Number(event.target.value) } })}>
      {s("weekdayNames").split(",").map((name, index) => <option key={name} value={index + 1}>{name}</option>)}
    </select></label>}
    {spec.kind === "onceAt" && <label>{s("dateTime")}<input type="datetime-local" value={onceTime} onChange={(event) => onOnceTime(event.target.value)} required /></label>}
    <p className="schedule-hint">{s("clockHint")}</p>
  </fieldset>;
}
