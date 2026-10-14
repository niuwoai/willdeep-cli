import { useState } from "react";
import { useQuery } from "@tanstack/react-query";
import type { Language } from "./i18n";
import { agentText } from "./agentLibraryMessages";

export type LocalAgentBinding = {provider_profile: string | null; model: string | null};
async function read<T>(path: string): Promise<T> {
  const response = await fetch(path);
  if (!response.ok) throw new Error(String(response.status));
  return await response.json() as T;
}
export function AgentModelBinding({language, binding, pending, onSave, onCancel}: {language: Language; binding: LocalAgentBinding; pending: boolean; onSave: (binding: LocalAgentBinding) => void; onCancel: () => void}) {
  const t = (key: Parameters<typeof agentText>[0]) => agentText(key,language);
  const [provider,setProvider] = useState(binding.provider_profile ?? "");
  const [model,setModel] = useState(binding.model ?? "");
  const providers = useQuery({queryKey:["agent-binding-providers"],queryFn:() => read<{providers:{id:string;provider:string;model:string}[]}>("/api/settings/model-routing")});
  const models = useQuery({queryKey:["agent-binding-models",provider],queryFn:() => read<{models:string[];warning:string|null}>(`/api/schedules/models?profile=${encodeURIComponent(provider)}`)});
  const options = [...new Set([...(models.data?.models ?? []),...(model ? [model] : [])])];
  const profiles = [...new Set([...(providers.data?.providers.map(item=>item.id) ?? []),...(provider ? [provider] : [])])];
  return <section className="agent-library-editor">
    <h2>{t("modelBinding")}</h2><p>{t("modelBindingHelp")}</p>
    <label>{t("provider")}<select aria-label={t("provider")} value={provider} disabled={pending} onChange={event=>{setProvider(event.target.value);setModel("");}}><option value="">{t("inherit")}</option>{profiles.map(id=><option value={id} key={id}>{id}</option>)}</select></label>
    <label>{t("model")}<select aria-label={t("model")} value={model} disabled={pending} onChange={event=>setModel(event.target.value)}><option value="">{t("inherit")}</option>{options.map(name=><option value={name} key={name}>{name}</option>)}</select></label>
    {(providers.error || models.error) && <p role="alert">{t("failed")}: {(providers.error || models.error)?.message}</p>}
    {models.data?.warning && <p>{models.data.warning}</p>}
    <div className="agent-library-actions"><button disabled={pending} onClick={()=>onSave({provider_profile:provider || null,model:model || null})}>{t("save")}</button><button disabled={pending} onClick={onCancel}>{t("cancel")}</button></div>
  </section>;
}
