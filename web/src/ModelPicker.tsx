// `/model` 的选择器。先读 Provider 档案，再加载所选档案的模型目录；
// 切换时把模型名与 Provider 身份一起写进当前会话。
//
// 外壳与授权模式面板同一套 disclosure：工具栏里一个按钮，点开是一个浮层。

import { useEffect, useRef, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { Check, ChevronDown, Cpu, Search, X } from "lucide-react";
import type { Language } from "./i18n";
import { modelText } from "./modelMessages";
import { changeModel, loadDefaultModel, loadModelProviders, loadProviderModels, ModelFailure, type ModelOption } from "./modelApi";
import { useInspectorDismiss } from "./useInspectorDismiss";
import "./permissionsPanel.css";

export function ModelPicker({ sessionId, language, current, currentProviderId, open, onOpenChange, onPicked, onApplied }: {
  /// 还没有会话时为空：这时列表照常能看，选中的结果由调用方带进第一条消息，
  /// 不在这里对服务端下手——一条谁都没开的会话不该为了选个模型就被建出来。
  sessionId?: string | null;
  language: Language;
  /// 会话当前的模型名；`null` 表示还没选，按 Provider 默认走。
  current: string | null;
  /// 已知当前模型所属的 Provider；没有时从模型列表推断。
  currentProviderId?: string | null;
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
  const [pending, setPending] = useState("");
  const [providerId, setProviderId] = useState("");
  const [search, setSearch] = useState("");
  const providerTouched = useRef(false);
  const [notice, setNotice] = useState<{ tone: "ok" | "error"; text: string } | null>(null);
  const providerSettings = useQuery({ queryKey: ["models", "providers"], queryFn: ({ signal }) => loadModelProviders(signal), enabled: open, staleTime: 60_000 });
  // 没选过模型时也要说清楚这一轮到底会用谁，而不是留一个空按钮让人猜。
  const defaults = useQuery({ queryKey: ["models", "default"], queryFn: ({ signal }) => loadDefaultModel(signal), enabled: open || !current });
  const providers = providerSettings.data?.providers ?? [];
  const fallback = defaults.data?.model?.model ?? null;
  // 远程页面改不了模型：把选择器置灰并写明原因，别让用户选完才知道。
  const locked = defaults.data?.writable === false;
  const configuredProviderId = currentProviderId && providers.some((provider) => provider.id === currentProviderId)
    ? currentProviderId : current ? providers.find((provider) => provider.model === current)?.id : undefined;
  const preferredProviderId = configuredProviderId ?? defaults.data?.model?.provider_id ?? providerSettings.data?.default_provider;
  useEffect(() => {
    if (!providers.length) return;
    setProviderId((previous) => {
      if (providerTouched.current && providers.some((provider) => provider.id === previous)) return previous;
      if (preferredProviderId && providers.some((provider) => provider.id === preferredProviderId)) return preferredProviderId;
      return providers.find((provider) => provider.id === providerSettings.data?.default_provider)?.id ?? providers[0].id;
    });
  }, [providers, preferredProviderId, providerSettings.data?.default_provider]);
  const selectedProviderId = providers.some((provider) => provider.id === providerId) ? providerId
    : preferredProviderId && providers.some((provider) => provider.id === preferredProviderId) ? preferredProviderId
    : providers.find((provider) => provider.id === providerSettings.data?.default_provider)?.id ?? providers[0]?.id ?? "";
  const selectedProvider = providers.find((provider) => provider.id === selectedProviderId);
  const models = useQuery({ queryKey: ["models", "provider", selectedProviderId], queryFn: ({ signal }) => loadProviderModels(selectedProviderId, signal), enabled: open && Boolean(selectedProviderId), staleTime: 60_000 });
  const query = search.trim().toLocaleLowerCase();
  const filteredModels = (models.data?.models ?? []).filter((model) => model.trim() && model.toLocaleLowerCase().includes(query)).map((model) => ({ provider_id: selectedProviderId, provider: selectedProvider?.provider ?? "", model }));
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
      setProviderId(option.provider_id);
      providerTouched.current = false;
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
        {providerSettings.isLoading && <p className="permissions-blocked">{t.loading}</p>}
        {providerSettings.error && <p role="alert" className="permissions-failure">{t.loadFailed}: {providerSettings.error.message} <button type="button" onClick={() => void providerSettings.refetch()}>{t.retry}</button></p>}
        {providerSettings.data && providers.length === 0 && <p className="permissions-blocked">{t.empty}</p>}
        {providers.length > 0 && <>
          <div className="model-cascade-controls">
            <label className="model-provider-field">
              <span>{t.provider}</span>
              <select aria-label={t.provider} value={selectedProviderId} onChange={(event) => { providerTouched.current = true; setProviderId(event.target.value); setSearch(""); }}>
                {providers.map((provider) => <option key={provider.id} value={provider.id}>{provider.id} · {provider.provider}{provider.id === providerSettings.data?.default_provider ? ` · ${t.defaultProvider}` : ""}</option>)}
              </select>
            </label>
            <label className="model-search-field">
              <span>{t.searchModels}</span>
              <span className="model-search-control"><Search size={15} aria-hidden="true" /><input type="search" aria-label={t.searchModels} placeholder={t.searchPlaceholder} value={search} onChange={(event) => setSearch(event.target.value)} disabled={!selectedProviderId || models.isLoading || Boolean(models.error)} /></span>
            </label>
          </div>
          {models.isLoading && <p className="permissions-blocked">{t.loadingModels}</p>}
          {models.error && <p role="alert" className="permissions-failure">{t.loadFailed}: {models.error.message} <button type="button" onClick={() => void models.refetch()}>{t.retry}</button></p>}
          {models.data?.warning && <p className="model-warning">{t.modelsWarning} {models.data.warning}</p>}
          {models.data && filteredModels.length === 0 && <p className="permissions-blocked">{query ? t.noSearchResults : t.providerEmpty}</p>}
          <section className="model-group" aria-label={selectedProvider?.id}>
            <h3 className="model-group-title">{selectedProvider?.id}<small>{selectedProvider?.provider}</small></h3>
            <div className="model-options-list">
              {filteredModels.map((option) => {
                const active = current === option.model && selectedProviderId === option.provider_id;
                const isDefault = !current && fallback === option.model && defaults.data?.model?.provider_id === option.provider_id;
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
            </div>
          </section>
        </>}

        <p className="permissions-blocked">{t.usage}</p>
      </div>
    </details>
  </aside>;
}
