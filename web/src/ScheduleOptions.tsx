import { useQuery } from "@tanstack/react-query";
import type { Language, Messages } from "./i18n";
import { scheduleText } from "./scheduleMessages";
import { scheduleJson, type ScheduleDraft, type ScheduleModels } from "./scheduleTypes";

export function ScheduleOptions({ draft, onChange, language, messages, disabled }: {
  draft: ScheduleDraft; onChange: (draft: ScheduleDraft) => void; language: Language; messages: Messages; disabled: boolean;
}) {
  const s = (key: Parameters<typeof scheduleText>[0]) => scheduleText(key, language);
  const models = useQuery({ queryKey: ["schedule-model-options"], queryFn: ({ signal }) => scheduleJson<ScheduleModels>("/api/settings/model-routing", { signal }), staleTime: 60_000 });
  const discovered = useQuery({ queryKey: ["schedule-provider-models", draft.profile], queryFn: ({ signal }) => scheduleJson<{ models: string[]; warning: string | null }>(`/api/schedules/models?${new URLSearchParams(draft.profile ? { profile: draft.profile } : {})}`, { signal }), staleTime: 60_000 });
  const providers = Array.from(new Set([draft.profile ?? "", ...(models.data?.providers.map((provider) => provider.id) ?? [])])).filter(Boolean);
  const choices = Array.from(new Set([draft.model ?? "", ...(discovered.data?.models ?? []), models.data?.root_model ?? "", ...(models.data?.providers.filter((provider) => provider.id === draft.profile).map((provider) => provider.model) ?? [])])).filter(Boolean);
  const number = (key: "max_turns" | "timeout_seconds" | "token_budget", value: string) => onChange({ ...draft, [key]: value ? Number(value) : null });
  return <details className="schedule-options"><summary>{s("budget")}</summary><fieldset disabled={disabled} className="schedule-fields">
    {draft.continue_in_session ? <p>{s("conversationHint")}</p> : <>
    <label>{s("profile")}<select value={draft.profile ?? ""} onChange={(event) => onChange({ ...draft, profile: event.target.value || null })}>
      <option value="">{s("inherited")}</option>{providers.map((provider) => <option key={provider} value={provider}>{provider}</option>)}
    </select></label>
    <label>{s("model")}<select value={draft.model ?? ""} onChange={(event) => onChange({ ...draft, model: event.target.value || null })}>
      <option value="">{s("inherited")}</option>{choices.map((model) => <option key={model} value={model}>{model}</option>)}
    </select></label>
    <label>{s("permission")}<select value={draft.approval_mode ?? ""} onChange={(event) => onChange({ ...draft, approval_mode: event.target.value || null })}>
      <option value="">{s("inherited")}</option><option value="strict">{messages.accessStrict}</option><option value="smart">{messages.accessSmart}</option><option value="workspace-write">{messages.accessWrite}</option><option value="full-access">{messages.accessFull}</option>
    </select></label>
    </>}
    <label>{s("maxTurns")}<input type="number" min="1" max="1000" value={draft.max_turns ?? ""} placeholder={s("runtimeDefault")} onChange={(event) => number("max_turns", event.target.value)} /></label>
    <label>{s("timeout")}<input type="number" min="1" max="86400" value={draft.timeout_seconds ?? ""} placeholder={s("runtimeDefault")} onChange={(event) => number("timeout_seconds", event.target.value)} /></label>
    <label>{s("tokens")}<input type="number" min="1000" max="10000000" value={draft.token_budget ?? ""} placeholder={s("runtimeDefault")} onChange={(event) => number("token_budget", event.target.value)} /></label>
    <label>{s("missed")}<select value={draft.missed_policy} onChange={(event) => onChange({ ...draft, missed_policy: event.target.value as ScheduleDraft["missed_policy"] })}>
      <option value="coalesce">{s("coalesce")}</option><option value="skip">{s("skipMissed")}</option>
    </select></label>
    <label>{s("failures")}<input type="number" min="1" max="100" value={draft.failure_limit} onChange={(event) => onChange({ ...draft, failure_limit: Number(event.target.value) })} required /></label>
    <label>{s("retention")}<input type="number" min="1" max="3650" value={draft.retention_days} onChange={(event) => onChange({ ...draft, retention_days: Number(event.target.value) })} required /></label>
  </fieldset>{discovered.error && <p role="alert">{s("error")}: {discovered.error.message}</p>}{discovered.data?.warning && <p>{discovered.data.warning}</p>}</details>;
}
