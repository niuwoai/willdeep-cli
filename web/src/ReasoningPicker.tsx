import { useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { ChevronDown, Gauge, X } from "lucide-react";
import type { Language } from "./i18n";
import { changeReasoningLevel, loadDefaultModel, loadReasoningProviders, loadSessionReasoningState } from "./modelApi";
import { reasoningText } from "./reasoningMessages";
import { useInspectorDismiss } from "./useInspectorDismiss";
import "./permissionsPanel.css";

export function ReasoningPicker({ sessionId, language, current, currentProviderId, onChanged, onApplied }: {
  sessionId?: string | null;
  language: Language;
  current: string | null;
  currentProviderId?: string | null;
  onChanged: (level: string | null) => void;
  onApplied?: (message: string) => void;
}) {
  const t = reasoningText(language);
  const panel = useInspectorDismiss();
  const queryClient = useQueryClient();
  const [open, setOpen] = useState(false);
  const [pending, setPending] = useState(false);
  const [notice, setNotice] = useState<{ tone: "ok" | "error"; text: string } | null>(null);
  const providers = useQuery({ queryKey: ["reasoning", "providers"], queryFn: ({ signal }) => loadReasoningProviders(signal), enabled: open, staleTime: 0 });
  const writable = useQuery({ queryKey: ["models", "default"], queryFn: ({ signal }) => loadDefaultModel(signal), enabled: open, staleTime: 30_000 });
  const session = useQuery({ queryKey: ["reasoning", "session", sessionId], queryFn: ({ signal }) => loadSessionReasoningState(sessionId!, signal), enabled: open && Boolean(sessionId), staleTime: 0 });
  const providerId = currentProviderId ?? session.data?.provider_id ?? null;
  const provider = providers.data?.providers.find((item) => item.id === providerId);
  const profile = provider?.reasoning ?? null;
  const levels = Object.keys(profile?.levels ?? {});
  const selected = session.isSuccess ? session.data.reasoning_level : current;
  const defaultLevel = profile?.default_level;
  const shown = selected || (defaultLevel ? t.providerDefault.replace("{level}", defaultLevel) : t.followDefault);

  async function apply(level: string | null) {
    if (!sessionId || pending) return;
    setPending(true);
    setNotice(null);
    try {
      await changeReasoningLevel(sessionId, level);
      onChanged(level);
      await queryClient.invalidateQueries({ queryKey: ["reasoning", "session", sessionId] });
      const message = t.saved;
      onApplied?.(message);
      setNotice({ tone: "ok", text: message });
    } catch (reason) {
      setNotice({ tone: "error", text: `${t.failed}: ${reason instanceof Error ? reason.message : String(reason)}` });
    } finally {
      setPending(false);
    }
  }

  return <aside className="conversation-permissions">
    <details ref={panel} className="conversation-disclosure" onToggle={(event) => { setOpen(event.currentTarget.open); if (event.currentTarget.open) setNotice(null); }}>
      <summary aria-label={`${t.title} · ${shown}`} title={`${t.title} · ${shown}`}>
        <Gauge size={16} /><span>{shown}</span><ChevronDown size={13} />
      </summary>
      <div className="conversation-inspector permissions-inspector model-inspector reasoning-inspector">
        <div className="inspector-heading">
          <div><h2>{t.title}</h2><p>{t.subtitle}</p></div>
          <div className="inspector-controls"><button className="inspector-icon" type="button" aria-label={t.close} title={t.close} onClick={() => { if (panel.current) panel.current.open = false; }}><X size={16} /></button></div>
        </div>
        {!sessionId && <p className="permissions-blocked">{t.noSession}</p>}
        {writable.data?.writable === false && <p className="permissions-blocked">{t.notWritable}</p>}
        {(providers.isLoading || session.isLoading) && <p className="permissions-blocked">{t.loading}</p>}
        {(providers.error || session.error || writable.error) && <p role="alert" className="permissions-failure">{t.loadFailed}: {(providers.error ?? session.error ?? writable.error)?.message}</p>}
        {notice && <p role={notice.tone === "error" ? "alert" : "status"} className={notice.tone === "error" ? "permissions-failure" : "permissions-notice"}>{notice.text}</p>}
        {sessionId && providers.isSuccess && !levels.length && <p className="permissions-blocked">{t.notConfigured}</p>}
        {sessionId && providers.isSuccess && (levels.length > 0 || selected !== null) && <>
          <label className="model-provider-field reasoning-level-field">
            <span>{t.title}</span>
            <select aria-label={t.title} value={selected ?? ""} disabled={pending || !sessionId || writable.data?.writable === false} onChange={(event) => void apply(event.target.value || null)}>
              <option value="">{defaultLevel ? t.providerDefault.replace("{level}", defaultLevel) : t.followDefault}</option>
              {selected && !levels.includes(selected) && <option value={selected}>{selected}</option>}
              {levels.map((level) => <option key={level} value={level}>{level}</option>)}
            </select>
          </label>
          {pending && <p className="permissions-blocked">{t.applying}</p>}
          {levels.length > 0 && <p className="permissions-blocked">{t.available}: {levels.join(", ")}</p>}
        </>}
      </div>
    </details>
  </aside>;
}
