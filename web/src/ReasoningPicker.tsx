import { useEffect, useRef, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { ChevronDown, Gauge, X } from "lucide-react";
import type { Language } from "./i18n";
import { changeReasoningLevel, loadDefaultModel, loadReasoningProviders, loadSessionReasoningState } from "./modelApi";
import type { ReasoningProfile } from "./modelApi";
import { reasoningText } from "./reasoningMessages";
import { useInspectorDismiss } from "./useInspectorDismiss";
import "./permissionsPanel.css";

/// 可识别的上游等级词表：只用来决定拉杆的档位顺序，不参与请求值。
/// 配置映射里的自定义名字若能按数字或这套词表排序就上拉杆，否则保留下拉。
const CANONICAL_LEVELS = ["minimal", "low", "medium", "high", "xhigh", "max"];

function sliderLevels(
  profile: ReasoningProfile | null,
  supplied: string[] | undefined,
  builtin: boolean,
): string[] | null {
  const levels = supplied?.length ? supplied : Object.keys(profile?.levels ?? {});
  if (levels.length < 2) return null;
  if (builtin) return levels;
  const values = profile?.levels ?? {};
  if (levels.every((name) => typeof values[name] === "number")) {
    return [...levels].sort((a, b) => Number(values[a]) - Number(values[b]));
  }
  if (levels.every((name) => CANONICAL_LEVELS.includes(name))) {
    return [...levels].sort((a, b) => CANONICAL_LEVELS.indexOf(a) - CANONICAL_LEVELS.indexOf(b));
  }
  return null;
}

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
  // 拖动中的档位（undefined = 跟随已确认值）。松手才发请求，失败回滚到这个值。
  const [draft, setDraft] = useState<string | null | undefined>(undefined);
  const inFlight = useRef(false);
  const queued = useRef<{ level: string | null; fromSlider: boolean } | null>(null);
  const keyCommit = useRef<number | null>(null);
  const providers = useQuery({ queryKey: ["reasoning", "providers"], queryFn: ({ signal }) => loadReasoningProviders(signal), enabled: open, staleTime: 0 });
  const writable = useQuery({ queryKey: ["models", "default"], queryFn: ({ signal }) => loadDefaultModel(signal), enabled: open, staleTime: 30_000 });
  const session = useQuery({ queryKey: ["reasoning", "session", sessionId], queryFn: ({ signal }) => loadSessionReasoningState(sessionId!, signal), enabled: open && Boolean(sessionId), staleTime: 0 });
  const providerId = currentProviderId ?? session.data?.provider_id ?? null;
  const provider = providers.data?.providers.find((item) => item.id === providerId);
  const profile = provider?.reasoning ?? null;
  const builtin = provider?.reasoning_builtin === true;
  const configured = Object.keys(profile?.levels ?? {});
  // 旧 daemon 不返回 reasoning_levels：沿用原来「只看配置映射」的行为。
  const levels = provider?.reasoning_levels?.length ? provider.reasoning_levels : configured;
  const slider = provider ? sliderLevels(profile, provider.reasoning_levels, builtin) : null;
  const selected = session.isSuccess ? session.data.reasoning_level : current;
  const defaultLevel = profile?.default_level;
  // 切到不认这个等级的 Provider 后，会话里的旧覆盖仍要看得见、能清掉，
  // 但不能假装它会在下一轮生效。旧 daemon 不返回 reasoning_levels 时
  // levels 就是配置映射，两边的判定一致。
  const stale = providers.isSuccess && !session.isLoading && selected !== null && !levels.includes(selected)
    ? selected
    : null;
  const levelName = (level: string) => {
    switch (level) {
      case "minimal": return t.levelMinimal;
      case "low": return t.levelLow;
      case "medium": return t.levelMedium;
      case "high": return t.levelHigh;
      case "xhigh": return t.levelXhigh;
      case "max": return t.levelMax;
      default: return level;
    }
  };
  const shown = stale
    ? `${levelName(stale)} · ${t.unavailableTag}`
    : selected ? levelName(selected) : defaultLevel ? t.providerDefault.replace("{level}", levelName(defaultLevel)) : t.followDefault;
  const stops: (string | null)[] = slider ? [null, ...slider] : [];
  // 不可用的旧覆盖单独成档，只用来显示和迁出。
  if (slider && selected && !slider.includes(selected)) stops.push(selected);
  const target = draft === undefined ? selected : draft;
  const index = Math.max(0, stops.indexOf(target));
  const showingStale = stale !== null && stops[index] === stale;
  const stopLabel = (stop: string | null) => stop === null
    ? defaultLevel ? t.providerDefault.replace("{level}", levelName(defaultLevel)) : t.followDefault
    : levelName(stop);
  const tickLabel = (stop: string | null) => stop === null ? t.levelDefaultShort : levelName(stop);
  // 保存中不再禁用控件：禁用会让聚焦的 range 失焦，也会吞掉拖动/按键，
  // 使下面的串行化队列永远收不到第二次选择。
  const disabled = !sessionId || writable.data?.writable === false;

  useEffect(() => {
    setDraft(undefined);
  }, [open]);

  async function apply(level: string | null, fromSlider = false) {
    if (!sessionId) return;
    // 请求串行化：拖动很快时只保留最后一个目标，不静默丢掉用户的选择。
    if (inFlight.current) {
      queued.current = { level, fromSlider };
      return;
    }
    inFlight.current = true;
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
      inFlight.current = false;
      setPending(false);
      const next = queued.current;
      queued.current = null;
      if (next) {
        // 队列里的最新目标还没落盘，先别把 thumb 弹回旧值。
        void apply(next.level, next.fromSlider);
      } else if (fromSlider) {
        setDraft(undefined);
      }
    }
  }

  function cancelKeyCommit() {
    if (keyCommit.current !== null) {
      window.clearTimeout(keyCommit.current);
      keyCommit.current = null;
    }
  }

  function commit() {
    cancelKeyCommit();
    const next = draft === undefined ? selected : draft;
    if (next === selected) {
      setDraft(undefined);
      return;
    }
    void apply(next, true);
  }

  // 方向键逐档调整：停 350ms 没有新按键再提交，避免每按一次都发请求。
  function scheduleKeyCommit() {
    cancelKeyCommit();
    keyCommit.current = window.setTimeout(() => {
      keyCommit.current = null;
      commit();
    }, 350);
  }

  useEffect(() => () => {
    if (keyCommit.current !== null) window.clearTimeout(keyCommit.current);
  }, []);

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
        {sessionId && providers.isSuccess && stale && <p className="permissions-blocked">{t.staleOverride.replace("{level}", levelName(stale))}</p>}
        {sessionId && providers.isSuccess && slider && <>
          {builtin && <p className="permissions-blocked">{t.builtinNotice}</p>}
          <div className="reasoning-slider">
            <div className="reasoning-slider-ticks" aria-hidden="true">
              {stops.map((stop, position) => <span key={`${stop ?? "default"}-${position}`} className={position === index ? "current" : undefined}>{tickLabel(stop)}</span>)}
            </div>
            <input
              type="range"
              min={0}
              max={stops.length - 1}
              step={1}
              value={index}
              disabled={disabled}
              aria-label={t.title}
              aria-valuetext={stopLabel(stops[index])}
              onChange={(event) => setDraft(stops[Number(event.target.value)] ?? null)}
              onPointerUp={commit}
              onKeyUp={scheduleKeyCommit}
              onBlur={commit}
            />
            <p className="reasoning-slider-current">{t.current}: {stopLabel(stops[index])}{builtin && stops[index] !== null && !showingStale ? ` · ${t.builtinTag}` : ""}</p>
            <p className="reasoning-slider-hint">{t.sliderHint}</p>
          </div>
          {pending && <p className="permissions-blocked">{t.applying}</p>}
        </>}
        {sessionId && providers.isSuccess && !slider && (levels.length > 0 || selected !== null) && <>
          <label className="model-provider-field reasoning-level-field">
            <span>{t.title}</span>
            <select aria-label={t.title} value={selected ?? ""} disabled={disabled} onChange={(event) => void apply(event.target.value || null)}>
              <option value="">{defaultLevel ? t.providerDefault.replace("{level}", levelName(defaultLevel)) : t.followDefault}</option>
              {selected && !levels.includes(selected) && <option value={selected}>{levelName(selected)}</option>}
              {levels.map((level) => <option key={level} value={level}>{levelName(level)}</option>)}
            </select>
          </label>
          {pending && <p className="permissions-blocked">{t.applying}</p>}
          {levels.length > 0 && <p className="permissions-blocked">{t.available}: {levels.map(levelName).join(", ")}</p>}
        </>}
      </div>
    </details>
  </aside>;
}
