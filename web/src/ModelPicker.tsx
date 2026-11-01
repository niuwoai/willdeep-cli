// `/model` 的选择器。列表来自 `/api/models`，点一个就直接切——模型名是可逆的，
// 切错了再点一次就是了。
//
// 外壳与授权模式面板同一套 disclosure：工具栏里一个按钮，点开是一个浮层。

import { useEffect, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { Check, ChevronDown, Cpu, X } from "lucide-react";
import type { Language } from "./i18n";
import { modelText } from "./modelMessages";
import { changeModel, loadModels, ModelFailure, type ModelOption } from "./modelApi";
import { useInspectorDismiss } from "./useInspectorDismiss";
import "./permissionsPanel.css";

export function ModelPicker({ sessionId, language, current, open, onOpenChange, onPicked }: {
  sessionId: string;
  language: Language;
  /// 会话当前的模型名；`null` 表示跟随 Provider 默认。
  current: string | null;
  open: boolean;
  onOpenChange: (open: boolean) => void;
  onPicked: (model: string) => void;
}) {
  const t = modelText(language);
  const panel = useInspectorDismiss();
  const models = useQuery({ queryKey: ["models"], queryFn: ({ signal }) => loadModels(signal), enabled: open });
  const [pending, setPending] = useState("");
  const [notice, setNotice] = useState<{ tone: "ok" | "error"; text: string } | null>(null);

  useEffect(() => {
    const node = panel.current;
    if (node && node.open !== open) node.open = open;
  }, [open, panel]);

  useEffect(() => { if (open) setNotice(null); }, [open]);

  async function pick(option: ModelOption) {
    if (pending) return;
    setPending(`${option.provider_id}:${option.model}`);
    setNotice(null);
    try {
      await changeModel(sessionId, option.model, option.provider_id);
      onPicked(option.model);
      setNotice({ tone: "ok", text: t.applied.replace("{model}", option.model) });
    } catch (reason) {
      const detail = reason instanceof ModelFailure ? reason.message : String(reason);
      setNotice({ tone: "error", text: `${t.failed}: ${detail}` });
    } finally {
      setPending("");
    }
  }

  // 按 Provider 配置名分组：同一个网关下的几个模型排在一起，用户按「这条会话配的
  // 是哪个 Provider」来找。
  const groups = new Map<string, ModelOption[]>();
  for (const option of models.data ?? []) {
    const list = groups.get(option.provider_id) ?? [];
    list.push(option);
    groups.set(option.provider_id, list);
  }

  return <aside className="conversation-permissions">
    <details ref={panel} className="conversation-disclosure" onToggle={(event) => onOpenChange(event.currentTarget.open)}>
      <summary aria-label={`${t.title}${current ? ` · ${current}` : ""}`} title={t.title}>
        <Cpu size={16} /><span>{current ?? t.title}</span><ChevronDown size={13} />
      </summary>
      <div className="conversation-inspector permissions-inspector model-inspector">
        <div className="inspector-heading">
          <div><h2>{t.title}</h2><p>{current ?? t.followDefault} · {t.subtitle}</p></div>
          <div className="inspector-controls">
            <button className="inspector-icon" type="button" aria-label={t.close} title={t.close} onClick={() => onOpenChange(false)}><X size={16} /></button>
          </div>
        </div>

        {notice && (
          <p role={notice.tone === "error" ? "alert" : "status"} className={notice.tone === "error" ? "permissions-failure" : "permissions-notice"}>
            {notice.text}
          </p>
        )}
        {models.isLoading && <p className="permissions-blocked">{t.loading}</p>}
        {models.error && <p role="alert" className="permissions-failure">{t.loadFailed}: {models.error.message}</p>}
        {models.data && !models.data.length && <p className="permissions-blocked">{t.empty}</p>}

        {[...groups.entries()].map(([providerId, options]) => (
          <section className="model-group" key={providerId}>
            <h3 className="model-group-title">{providerId}<small>{options[0]?.provider}</small></h3>
            {options.map((option) => {
              const active = current === option.model;
              const busy = pending === `${option.provider_id}:${option.model}`;
              return (
                <button
                  key={`${option.provider_id}:${option.model}`}
                  type="button"
                  className={`permissions-option${active ? " selected" : ""}`}
                  disabled={Boolean(pending)}
                  aria-pressed={active}
                  onClick={() => void pick(option)}
                >
                  <span className="model-option-mark" aria-hidden="true">{active ? <Check size={13} /> : null}</span>
                  <strong>{option.model}</strong>
                  {busy && <small>{t.applying}</small>}
                </button>
              );
            })}
          </section>
        ))}

        <p className="permissions-blocked">{t.usage}</p>
      </div>
    </details>
  </aside>;
}
