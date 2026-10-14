import { useRef, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { agentLibraryCall, nextAgentVersion } from "./agentLibraryModel";
import type { Language } from "./i18n";
import { agentText, type AgentTextKey } from "./agentLibraryMessages";
import "./agentLibrary.css";
import { AgentModelBinding, type LocalAgentBinding } from "./AgentModelBinding";

type Definition = { schema: string; definition_id: string; version: string; name: Record<string, string>; purpose: string; base_role: string; instructions: string; input_modalities: string[]; tools: string[]; skills: {id: string; content: string; sha256: string; required: boolean}[]; forked_from: string | null };
type Package = { definition: Definition; sha256: string; signature: {key_id: string; value: string} | null };
type Item = { package: Package; scope: string; compatible: boolean; incompatible_reason: string | null; optional_skills_missing: string[]; installation: {origin: string; enabled: boolean; pinned: boolean; dirty: boolean; binding: LocalAgentBinding} };
type Library = { items: Item[]; account: {origin: string; user_id: string; tenant_id: string} | null };
type Index = Pick<Definition, "definition_id" | "version" | "name" | "purpose" | "base_role"> & {sha256: string};

export function AgentLibrary({language, onTrial}: {language: Language; onTrial: (prompt: string) => void}) {
  const t = (key: AgentTextKey) => agentText(key, language);
  const client = useQueryClient();
  const library = useQuery({queryKey: ["agent-library"], queryFn: () => agentLibraryCall<Library>("list")});
  const catalog = useQuery({queryKey: ["agent-catalog"], queryFn: () => agentLibraryCall<{items: Index[]}>("catalog"), enabled: false});
  const [editor, setEditor] = useState<string | null>(null);
  const [preview, setPreview] = useState<Package | null>(null);
  const [history, setHistory] = useState<Package[]>([]);
  const [bindingItem,setBindingItem] = useState<Item|null>(null);
  const [result, setResult] = useState<string>("");
  const file = useRef<HTMLInputElement>(null);
  const mutation = useMutation({
    mutationFn: ({operation, params}: {operation: string; params?: unknown}) => agentLibraryCall<unknown>(operation, params),
    onSuccess: async (value) => { setResult(JSON.stringify(value, null, 2)); await client.invalidateQueries({queryKey: ["agent-library"]}); },
  });
  const act = (operation: string, params?: unknown) => mutation.mutate({operation, params});
  const items = library.data?.items ?? [];
  const displayName = (definition: Pick<Definition, "name" | "definition_id">) => definition.name[language === "zh-CN" ? "zh-Hans" : language] ?? definition.name.en ?? definition.definition_id;
  const create = () => setEditor(JSON.stringify({schema: "willdeep.agent-definition.v1", definition_id: "user/my-agent", version: "1.0.0", name: {en: t("name"), [language === "zh-CN" ? "zh-Hans" : language]: t("name")}, purpose: t("purpose"), base_role: "reviewer", instructions: t("instructions"), input_modalities: ["text"], tools: ["workspace.read", "repository.inspect"], skills: [], forked_from: null}, null, 2));
  const edit = (definition: Definition) => setEditor(JSON.stringify({...definition, version: nextAgentVersion(definition.version)}, null, 2));
  const fork = (item: Item) => {
    const newId = window.prompt(t("id"), `user/${item.package.definition.definition_id.split("/").at(-1)}-copy`);
    if (newId) act("fork", {id: item.package.definition.definition_id, new_id: newId});
  };
  const exported = async (item: Item) => {
    const value = await mutation.mutateAsync({operation: "export", params: {id: item.package.definition.definition_id}});
    const url = URL.createObjectURL(new Blob([JSON.stringify(value, null, 2)], {type: "application/json"}));
    const link = document.createElement("a"); link.href = url; link.download = `${item.package.definition.definition_id.replaceAll("/", "-")}.json`; link.click(); URL.revokeObjectURL(url);
  };
  const button = (key: AgentTextKey, action: () => void, disabled = false) => <button type="button" disabled={disabled || mutation.isPending} onClick={action}>{t(key)}</button>;
  return <main className="agent-library">
    <header><h1>{t("title")}</h1><p>{t("intro")}</p></header>
    <div className="agent-library-actions">
      {button("create", create)}{button("import", () => file.current?.click())}{button("catalog", () => { void catalog.refetch(); })}
      {button("sync", () => act("sync"), !library.data?.account)}
      {library.data?.account && button("logout", () => act("logout"))}
      <input ref={file} type="file" accept="application/json,.json" hidden onChange={(event) => { const selected = event.target.files?.[0]; if (selected) void selected.text().then(package_text => act("import", {package_text})); event.target.value = ""; }} />
    </div>
    <p className="agent-library-account">{library.data?.account ? `${library.data.account.origin} · ${library.data.account.user_id} / ${library.data.account.tenant_id}` : t("loginHelp")}</p>
    {(mutation.error || library.error || catalog.error) && <div role="alert">{t("failed")}: {(mutation.error || library.error || catalog.error)?.message}</div>}
    {(mutation.isPending || library.isLoading || catalog.isFetching) && <p role="status">{t("busy")}</p>}
    {editor !== null && <section className="agent-library-editor">
      <h2>{t("advanced")}</h2><p>{t("editorHelp")}</p>
      <textarea aria-label={t("advanced")} value={editor} onChange={event => setEditor(event.target.value)} spellCheck={false} />
      {button("save", () => { void mutation.mutateAsync({operation: "save", params: {definition_text: editor}}).then(() => setEditor(null)).catch(() => setResult(t("failed"))); })}{button("cancel", () => setEditor(null))}
    </section>}
    {bindingItem && <AgentModelBinding key={`${bindingItem.scope}/${bindingItem.package.definition.definition_id}`} language={language} binding={bindingItem.installation.binding ?? {provider_profile:null,model:null}} pending={mutation.isPending} onCancel={()=>setBindingItem(null)} onSave={binding=>{void mutation.mutateAsync({operation:"bind",params:{id:bindingItem.package.definition.definition_id,binding}}).then(()=>setBindingItem(null)).catch(()=>setResult(t("failed")));}} />}
    {!items.length && !library.isLoading && <p>{t("empty")}</p>}
    <section className="agent-library-grid">
      {items.map(item => {
        const definition = item.package.definition;
        const role = definition.base_role as AgentTextKey;
        return <article key={`${item.scope}/${definition.definition_id}`} className="agent-library-card">
          <h2>{displayName(definition)}</h2><p>{definition.purpose}</p>
          <div className="agent-library-badges"><span>{t(item.installation.origin as AgentTextKey)}</span><span>{t(role)}</span><span>{t(item.installation.enabled ? "enabled" : "disabled")}</span>{item.installation.pinned && <span>{t("pinned")}</span>}{item.scope === "private" && <span>{t(item.installation.dirty ? "dirty" : "synced")}</span>}</div>
          <code>{definition.definition_id} · {definition.version}</code><code className="agent-revision">{item.package.sha256}</code>
          {(item.installation.binding?.provider_profile || item.installation.binding?.model) && <p>{t("modelBinding")}: {[item.installation.binding.provider_profile,item.installation.binding.model].filter(Boolean).join(" / ")}</p>}
          {!item.compatible && <p role="status">{t("incompatible")}: {item.incompatible_reason}</p>}
          {!!item.optional_skills_missing.length && <p>{t("optionalMissing")}: {item.optional_skills_missing.join(", ")}</p>}
          <div className="agent-library-actions">
            {item.installation.origin !== "official" && button("edit", () => edit(definition), item.installation.pinned)}
            {button("fork", () => fork(item))}{button("export", () => { void exported(item).catch(() => setResult(t("failed"))); })}
            {button("modelBinding",()=>setBindingItem(item))}
            {button(item.installation.pinned ? "unpin" : "pin", () => act("pin", {id: definition.definition_id, value: !item.installation.pinned}))}
            {button(item.installation.enabled ? "disable" : "enable", () => act("enable", {id: definition.definition_id, value: !item.installation.enabled}))}
            {item.installation.origin === "local" && button("share", () => act("share", {id: definition.definition_id}), !library.data?.account)}
            {button("history", () => { void mutation.mutateAsync({operation: "history", params: {id: definition.definition_id}}).then(value => setHistory(value as Package[])).catch(() => setResult(t("failed"))); })}
            {button("trial", () => { const task = window.prompt(t("trialTask")); if (task) onTrial(t("trialPrompt").replace("{id}", definition.definition_id).replace("{task}", task)); }, !item.compatible || !item.installation.enabled)}
            {button("remove", () => { if (window.confirm(t("deleteConfirm"))) act("delete", {id: definition.definition_id}); })}
          </div>
        </article>;
      })}
    </section>
    {catalog.data && <section><h2>{t("catalog")}</h2>{catalog.data.items.map(item => <article className="agent-library-card" key={item.definition_id}><h3>{displayName(item)}</h3><p>{item.purpose}</p><code>{item.version} · {item.sha256}</code>{button("preview", () => { void mutation.mutateAsync({operation: "preview", params: {id: item.definition_id, revision: item.sha256}}).then(value => setPreview(value as Package)).catch(() => setResult(t("failed"))); })}</article>)}</section>}
    {preview && <section className="agent-library-editor"><h2>{t("newRevision")}</h2><pre>{JSON.stringify(preview.definition, null, 2)}</pre>{items.find(item => item.package.definition.definition_id === preview.definition.definition_id) && <><h3>{t("currentRevision")}</h3><pre>{JSON.stringify(items.find(item => item.package.definition.definition_id === preview.definition.definition_id)?.package.definition, null, 2)}</pre></>}{button("install", () => act("install", {id: preview.definition.definition_id, revision: preview.sha256}))}{button("cancel", () => setPreview(null))}</section>}
    {!!history.length && <section><h2>{t("history")}</h2>{history.map(item => <p key={item.sha256}><code>{item.definition.definition_id} · {item.definition.version} · {item.sha256}</code>{button("rollback", () => act("rollback", {id: item.definition.definition_id, revision: item.sha256}))}</p>)}</section>}
    {result && <details><summary>{t("done")}</summary><pre>{result}</pre></details>}
  </main>;
}
