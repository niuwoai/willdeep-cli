// `/model` 的选择器。列表来自 `/api/models`，点一个就直接切——模型名是可逆的，
// 切错了再点一次就是了。
//
// 外壳与授权模式面板同一套 disclosure：工具栏里一个按钮，点开是一个浮层。

import { useEffect, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { Check, ChevronDown, Cpu, X } from "lucide-react";
import type { Language } from "./i18n";
import { modelText } from "./modelMessages";
import { changeModel, loadDefaultModel, loadModels, ModelFailure, type ModelOption } from "./modelApi";
import { useInspectorDismiss } from "./useInspectorDismiss";
import "./permissionsPanel.css";

export function ModelPicker({ sessionId, language, current, open, onOpenChange, onPicked, onApplied }: {
  /// 还没有会话时为空：这时列表照常能看，选中的结果由调用方带进第一条消息，
  /// 不在这里对服务端下手——一条谁都没开的会话不该为了选个模型就被建出来。
  sessionId?: string | null;
  language: Language;
  /// 会话当前的模型名；`null` 表示还没选，按 Provider 默认走。
  current: string | null;
  open: boolean;
  onOpenChange: (open: boolean) => void;
  /// 选中的模型与它的 Provider 档案；会话已存在时服务端已经切好了。
  onPicked: (model: string, providerId: string) => void;
  /// 切换成功后调用：弹窗会被收起，成功提示交给常驻的宿主显示，
  /// 否则提示跟着面板一起消失，看起来像没反应。
  onApplied?: (message: string) => void;
}) {
  const t = modelText(language);
  const panel = useInspectorDismiss();
  const models = useQuery({ queryKey: ["models"], queryFn: ({ signal }) => loadModels(signal), enabled: open });
  // 没选过模型时也要说清楚这一轮到底会用谁，而不是留一个空按钮让人猜。
  const defaults = useQuery({ queryKey: ["models", "default"], queryFn: ({ signal }) => loadDefaultModel(signal), enabled: open || !current });
  const [pending, setPending] = useState("");
  const [notice, setNotice] = useState<{ tone: "ok" | "error"; text: string } | null>(null);
  const fallback = defaults.data?.model?.model ?? null;
  // 远程页面改不了模型：把选择器置灰并写明原因，别让用户选完才知道。
  const locked = defaults.data?.writable === false;
  const shown = current ?? fallback ?? t.title;

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
      // 有会话才写服务端；没有的话这一选只是草稿，跟着第一条消息一起交出去。
      if (sessionId) await changeModel(sessionId, option.model, option.provider_id);
      onPicked(option.model, option.provider_id);
      const text = t.applied.replace("{model}", option.model);
      // 成功就收起面板：模型名已经写进工具栏的按钮里，再挂着反而挡住对话。
      // 失败时保持在面板里，用户要能看到原因并重试。
      onApplied?.(text);
      onOpenChange(false);
      setNotice({ tone: "ok", text });
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
      <summary aria-label={`${t.title} · ${shown}`} title={`${t.title} · ${shown}`}>
        <Cpu size={16} /><span>{shown}</span><ChevronDown size={13} />
      </summary>
      <div className="conversation-inspector permissions-inspector model-inspector">
        <div className="inspector-heading">
          <div>
            <h2>{t.title}</h2>
            <p>{locked ? t.notWritable : `${current ? current : fallback ? t.defaultModel.replace("{model}", fallback) : t.followDefault} · ${t.subtitle}`}</p>
          </div>
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
              const isDefault = !current && fallback === option.model;
              const busy = pending === `${option.provider_id}:${option.model}`;
              return (
                <button
                  key={`${option.provider_id}:${option.model}`}
                  type="button"
                  className={`permissions-option${active || isDefault ? " selected" : ""}`}
                  disabled={Boolean(pending) || locked}
                  aria-pressed={active || isDefault}
                  onClick={() => void pick(option)}
                >
                  <span className="model-option-mark" aria-hidden="true">{active || isDefault ? <Check size={13} /> : null}</span>
                  <strong>{option.model}</strong>
                  {isDefault && <small>{t.defaultLabel}</small>}
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
