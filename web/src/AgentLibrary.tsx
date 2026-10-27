import { useCallback, useEffect, useRef, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { agentLibraryCall, nextAgentVersion } from "./agentLibraryModel";
import type { Language } from "./i18n";
import { agentText, type AgentTextKey } from "./agentLibraryMessages";
import "./agentLibrary.css";
import { AgentModelBinding, type LocalAgentBinding } from "./AgentModelBinding";
import { AgentRsiPanel, type RsiPackage } from "./AgentRsiPanel";
import { AgentSyncConflicts, type SyncConflict } from "./AgentSyncConflicts";
import { FileText, MoreHorizontal, Search, FlaskConical } from "lucide-react";
import { allowAgentNavigation } from "./agentNavigation";

type Definition = { schema: string; definition_id: string; version: string; name: Record<string, string>; purpose: string; base_role: string; instructions: string; input_modalities: string[]; tools: string[]; skills: {id: string; content: string; sha256: string; required: boolean}[]; forked_from: string | null };
type Package = { definition: Definition; sha256: string; signature: {key_id: string; value: string} | null };
type Item = { package: Package; scope: string; compatible: boolean; incompatible_reason: string | null; optional_skills_missing: string[]; installation: {origin: string; enabled: boolean; pinned: boolean; dirty: boolean; binding: LocalAgentBinding} };
type Library = { items: Item[]; account: {origin: string; user_id: string; tenant_id: string} | null };
type Index = Pick<Definition, "definition_id" | "version" | "name" | "purpose" | "base_role"> & {sha256: string; discovery?: {category: string; rank: number; summary: Record<string, string>; example: Record<string, string>} | null};

export function AgentLibrary({language, onTrial, mode = "library", onNavigate, onDirty}: {language: Language; onTrial: (prompt: string) => void; mode?: "library" | "experiments"; onNavigate?: (mode: "library" | "experiments") => void; onDirty?: (dirty: boolean) => void}) {
  const t = (key: AgentTextKey) => agentText(key, language);
  const client = useQueryClient();
  const displayedAccount = useRef("");
  const library = useQuery({queryKey: ["agent-library"], queryFn: () => agentLibraryCall<Library>("list")});
  const catalog = useQuery({queryKey: ["agent-catalog"], queryFn: async () => {
    const value = await agentLibraryCall<{items: Index[]}>("catalog");
    // A response that is not a catalog (for example a library listing from an
    // older endpoint) must surface as unavailable rather than crash rendering.
    if (!Array.isArray(value?.items) || value.items.some(item => typeof item?.name !== "object" || item.name === null)) {
      throw new Error(t("catalogUnavailable"));
    }
    return value;
  }, enabled: mode === "library", staleTime: 60_000, retry: 1});
  const [editor, setEditor] = useState<string | null>(null);
  const [preview, setPreview] = useState<Package | null>(null);
  const [history, setHistory] = useState<Package[]>([]);
  const [bindingItem,setBindingItem] = useState<Item|null>(null);
  const [result, setResult] = useState<string>("");
  const [conflicts, setConflicts] = useState<{account: string; items: SyncConflict[]} | null>(null);
  const [syncSummary,setSyncSummary] = useState<{account: string; text: string} | null>(null);
  const [rsi, setRsi] = useState<{baseline: RsiPackage | null; account: string} | null>(null);
  const [tab, setTab] = useState<"installed" | "curated" | null>(null);
  const [search, setSearch] = useState("");
  const [category, setCategory] = useState("");
  const [source, setSource] = useState("all");
  const [selected, setSelected] = useState<string | null>(null);
  const [viewMode, setViewMode] = useState(mode);
  if (viewMode !== mode) {setViewMode(mode); setEditor(null);}
  const [candidateDirty, setCandidateDirty] = useState(false);
  const draftChanged = useCallback((dirty: boolean) => setCandidateDirty(dirty), []);
  useEffect(() => {onDirty?.(candidateDirty || editor !== null); return () => onDirty?.(false);}, [candidateDirty, editor, onDirty]);
  const file = useRef<HTMLInputElement>(null);
  const mutation = useMutation({
    mutationFn: ({operation, params}: {operation: string; params?: unknown}) => agentLibraryCall<unknown>(operation, params),
    onMutate: () => ({account:accountKey}),
    onSuccess: async (value, {operation, params}, context) => {
      if (context?.account !== displayedAccount.current) {
        await client.invalidateQueries({queryKey:["agent-library"]}); return;
      }
      setResult(JSON.stringify(value, null, 2));
      if (operation === "sync") setConflicts({account: accountKey, items: (value as {conflict_details?: SyncConflict[]}).conflict_details ?? []});
      if (operation === "resolve") {
        setConflicts(previous => previous ? {...previous, items: previous.items.filter(item => item.definition_id !== (params as {id: string}).id)} : null);
        const resolved=value as {deleted?:boolean; version?:string};
        setSyncSummary({account:accountKey,text:resolved.deleted ? t("resolveDeleted") : t("resolveCompleted").replace("{version}",resolved.version ?? "")});
      }
      if (operation === "logout") {setRsi(null); setConflicts(null); client.removeQueries({queryKey: ["agent-rsi"]});}
      await client.invalidateQueries({queryKey: ["agent-library"]});
    },
  });
  const act = (operation: string, params?: unknown) => mutation.mutate({operation, params});
  const items = library.data?.items ?? [];
  const activeTab = tab ?? (items.length ? "installed" : "curated");
  const accountKey = JSON.stringify(library.data?.account ?? null);
  const resetMutation = mutation.reset;
  useEffect(() => {resetMutation();}, [accountKey, resetMutation]);
  displayedAccount.current = accountKey;
  const [viewAccount, setViewAccount] = useState(accountKey);
  if (viewAccount !== accountKey) {
    setViewAccount(accountKey); setEditor(null); setPreview(null); setHistory([]); setBindingItem(null); setRsi(null); setResult(""); setSelected(null); setSearch(""); setSource("all");
  }
  if (conflicts && conflicts.account !== accountKey) setConflicts(null);
  if (syncSummary && syncSummary.account !== accountKey) setSyncSummary(null);
  const displayName = (definition: Pick<Definition, "name" | "definition_id">) => definition.name[language === "zh-CN" ? "zh-Hans" : language] ?? definition.name.en ?? definition.definition_id;
  const localized = (values: Record<string, string>) => values[language === "zh-CN" ? "zh-Hans" : language] ?? values.en ?? Object.values(values)[0] ?? "";
  const catalogItems = catalog.isError ? [] : catalog.data?.items ?? [];
  const categories = Array.from(new Set(catalogItems.flatMap(item => item.discovery ? [item.discovery.category] : [])));
  const recommendations = catalogItems.filter(item => (!category || item.discovery?.category === category) && `${displayName(item)} ${item.purpose} ${item.discovery ? localized(item.discovery.summary) : ""} ${item.definition_id}`.toLocaleLowerCase().includes(search.toLocaleLowerCase())).sort((a, b) => (a.discovery?.rank ?? Number.MAX_SAFE_INTEGER) - (b.discovery?.rank ?? Number.MAX_SAFE_INTEGER));
  const scopedMutation = async (operation: string, params: unknown, done: (value: unknown) => void) => {
    const scope = displayedAccount.current;
    try {const value = await mutation.mutateAsync({operation, params}); if (scope === displayedAccount.current) done(value);}
    catch {if (scope === displayedAccount.current) setResult(t("failed"));}
  };
  const filtered = items.filter(item => (source === "all" || item.installation.origin === source) && `${displayName(item.package.definition)} ${item.package.definition.purpose} ${item.package.definition.definition_id}`.toLocaleLowerCase().includes(search.toLocaleLowerCase()));
  const current = filtered.find(item => `${item.scope}/${item.package.definition.definition_id}` === selected) ?? filtered[0];
  const experimentBaseline = rsi?.account === accountKey ? items.find(item => item.package.sha256 === rsi.baseline?.sha256)?.package ?? null : null;
  const allowDiscard = () => allowAgentNavigation(candidateDirty || editor !== null, language);
  const openExperiment = (baseline: RsiPackage | null) => {if (!allowDiscard()) return; setEditor(null); setRsi({baseline, account: accountKey}); onNavigate?.("experiments");};
  const create = () => {if (allowDiscard()) setEditor(JSON.stringify({schema: "willdeep.agent-definition.v1", definition_id: "user/my-agent", version: "1.0.0", name: {en: t("name"), [language === "zh-CN" ? "zh-Hans" : language]: t("name")}, purpose: t("purpose"), base_role: "reviewer", instructions: t("instructions"), input_modalities: ["text"], tools: ["workspace.read", "repository.inspect"], skills: [], forked_from: null}, null, 2));};
  const edit = (definition: Definition) => {if (allowDiscard()) setEditor(JSON.stringify({...definition, version: nextAgentVersion(definition.version)}, null, 2));};
  const fork = (item: Item, experiment = false) => {
    if (experiment && !allowDiscard()) return;
    const newId = window.prompt(t("id"), `user/${item.package.definition.definition_id.split("/").at(-1)}-copy`);
    if (newId) void scopedMutation("fork", {id: item.package.definition.definition_id, new_id: newId}, value => { if (experiment) openExperiment(value as Package); });
  };
  const exported = async (item: Item) => {
    const scope = displayedAccount.current;
    try {
      const value = await mutation.mutateAsync({operation: "export", params: {id: item.package.definition.definition_id}});
      if (scope !== displayedAccount.current) return;
      const url = URL.createObjectURL(new Blob([JSON.stringify(value, null, 2)], {type: "application/json"}));
      const link = document.createElement("a"); link.href = url; link.download = `${item.package.definition.definition_id.replaceAll("/", "-")}.json`; link.click(); URL.revokeObjectURL(url);
    } catch {if (scope === displayedAccount.current) setResult(t("failed"));}
  };
  const importFile = async (selected: File) => {
    const scope = displayedAccount.current;
    try {const packageText = await selected.text(); if (scope === displayedAccount.current) act("import", {package_text:packageText});}
    catch {if (scope === displayedAccount.current) setResult(t("failed"));}
  };
  const button = (key: AgentTextKey, action: () => void, disabled = false) => <button type="button" disabled={disabled || mutation.isPending} onClick={action}>{t(key)}</button>;
  if (mode === "experiments") return <main className="agent-library agent-experiment-workspace">
    <header className="agent-page-header"><div><h1>{t("experimentTitle")}</h1><p>{t("experimentSubtitle")}</p></div><div className="agent-header-controls"><span className="agent-account-pill">{t(library.data?.account ? "connected" : "localOnly")}</span><label className="rsi-baseline-select"><select aria-label={t("chooseBaseline")} value={experimentBaseline?.sha256 ?? ""} onChange={event => {if (allowDiscard()) setRsi({baseline:items.find(item => item.package.sha256 === event.target.value)?.package ?? null, account:accountKey});}}><option value="">{t("chooseBaseline")}</option>{items.filter(item => item.compatible).map(item => <option key={`${item.scope}/${item.package.sha256}`} value={item.package.sha256}>{displayName(item.package.definition)} · {item.package.definition.version}</option>)}</select></label></div></header>
    {library.error && <p role="alert">{t("failed")}: {library.error.message}</p>}
    <AgentRsiPanel key={`${accountKey}/${experimentBaseline?.sha256 ?? "list"}`} accountScope={accountKey} language={language} baseline={experimentBaseline} definitionNames={Object.fromEntries(items.map(item => [item.package.definition.definition_id,displayName(item.package.definition)]))} onDirty={draftChanged} onClose={() => {if (allowDiscard()) onNavigate?.("library");}} />
  </main>;
  return <main className="agent-library">
    <header className="agent-page-header"><div><h1>{t("title")}</h1><p>{t("intro")}</p></div><div className="agent-header-controls agent-library-header-actions"><button className="agent-primary" type="button" disabled={mutation.isPending} onClick={create}>{t("create")}</button><details className="agent-overflow"><summary aria-label={t("libraryActions")}><MoreHorizontal size={20}/></summary><div>{button("import", () => file.current?.click())}{button("sync", () => act("sync"), !library.data?.account)}{library.data?.account && button("logout", () => act("logout"))}</div></details></div></header>
    <input ref={file} type="file" accept="application/json,.json" hidden onChange={(event) => { const selected = event.target.files?.[0]; if (selected) void importFile(selected); event.target.value = ""; }} />
    <details className="agent-library-account"><summary>{t(library.data?.account ? "connected" : "localOnly")} · {t("accountDetails")}</summary><p>{library.data?.account ? `${library.data.account.origin} · ${library.data.account.user_id} / ${library.data.account.tenant_id}` : t("loginHelp")}</p></details>
    {(mutation.error || library.error || catalog.error) && <div role="alert">{t("failed")}: {(mutation.error || library.error || catalog.error)?.message}</div>}
    {(mutation.isPending || library.isLoading || catalog.isFetching) && <p role="status">{t("busy")}</p>}
    {syncSummary?.account === accountKey && <p role="status">{syncSummary.text}</p>}
    {conflicts?.account === accountKey && !!conflicts.items.length && <AgentSyncConflicts conflicts={conflicts.items} language={language} pending={mutation.isPending} onResolve={params=>act("resolve",{...params,expected_account:library.data?.account})} />}
    {editor !== null && <section className="agent-library-editor">
      <h2>{t("advanced")}</h2><p>{t("editorHelp")}</p>
      <textarea aria-label={t("advanced")} value={editor} disabled={mutation.isPending} onChange={event => setEditor(event.target.value)} spellCheck={false} />
      {button("save", () => { void scopedMutation("save", {definition_text: editor}, () => setEditor(null)); })}{button("cancel", () => setEditor(null))}
    </section>}
    {!onNavigate && rsi?.account === accountKey && <AgentRsiPanel key={`${accountKey}/${rsi.baseline?.sha256 ?? "list"}`} accountScope={accountKey} language={language} baseline={rsi.baseline} onClose={() => setRsi(null)} />}
    {bindingItem && <AgentModelBinding key={`${accountKey}/${bindingItem.scope}/${bindingItem.package.definition.definition_id}`} language={language} binding={bindingItem.installation.binding ?? {provider_profile:null,model:null}} pending={mutation.isPending} onCancel={()=>setBindingItem(null)} onSave={binding=>{void scopedMutation("bind",{id:bindingItem.package.definition.definition_id,binding},()=>setBindingItem(null));}} />}
    <div className="agent-library-actions" aria-label={t("title")}><button type="button" aria-pressed={activeTab === "installed"} onClick={() => setTab("installed")}>{t("installed")} · {items.length}</button><button type="button" aria-pressed={activeTab === "curated"} onClick={() => setTab("curated")}>{t("curated")}</button></div>
    {activeTab === "installed" && <>
    {!items.length && !library.isLoading && <p>{t("empty")}</p>}
    <div className="agent-library-filters"><label><Search size={18} /><input aria-label={t("search")} placeholder={t("search")} value={search} onChange={event => setSearch(event.target.value)} /></label><select aria-label={t("sourceFilter")} value={source} onChange={event => setSource(event.target.value)}><option value="all">{t("allSources")}</option>{(["local", "official", "private"] as const).map(origin => <option key={origin} value={origin}>{t(origin)}</option>)}</select></div>
    {!filtered.length && !!items.length && <p>{t("noMatches")}</p>}
    {!!items.length && <section className="agent-library-split">
      <div className="agent-library-list">{filtered.map(item => <button type="button" className={current === item ? "selected" : ""} aria-pressed={current === item} key={`${item.scope}/${item.package.definition.definition_id}`} onClick={() => setSelected(`${item.scope}/${item.package.definition.definition_id}`)}><FileText size={21} /><span><strong>{displayName(item.package.definition)}</strong><small>{t(item.installation.origin as AgentTextKey)} · {item.package.definition.version}</small></span></button>)}</div>
      <div className="agent-library-grid">{current ? [current].map(item => {
        const definition = item.package.definition;
        const role = definition.base_role as AgentTextKey;
        const official = item.installation.origin === "official" || Boolean(item.package.signature);
        return <article key={`${item.scope}/${definition.definition_id}`} className="agent-library-card">
          <h2>{displayName(definition)}</h2><p>{definition.purpose}</p>
          <div className="agent-library-badges"><span>{t(item.installation.origin as AgentTextKey)}</span><span>{t(role)}</span><span>{t(item.installation.enabled ? "enabled" : "disabled")}</span>{item.installation.pinned && <span>{t("pinned")}</span>}{item.scope === "private" && <span>{t(item.installation.dirty ? "dirty" : "synced")}</span>}</div>
          <p className="agent-version">{definition.version}</p>
          {(item.installation.binding?.provider_profile || item.installation.binding?.model) && <p>{t("modelBinding")}: {[item.installation.binding.provider_profile,item.installation.binding.model].filter(Boolean).join(" / ")}</p>}
          {!item.compatible && <p role="status">{t("incompatible")}: {item.incompatible_reason}</p>}
          {!!item.optional_skills_missing.length && <p>{t("optionalMissing")}: {item.optional_skills_missing.join(", ")}</p>}
          <div className="agent-library-actions agent-detail-actions">
            {official ? <button className="agent-primary" type="button" disabled={mutation.isPending} onClick={() => fork(item, true)}>{t("forkExperiment")}</button> : <button className="agent-primary" type="button" disabled={mutation.isPending || !item.compatible} onClick={() => openExperiment(item.package)}><FlaskConical size={18} />{t("rsiTitle")}</button>}
          <details className="agent-overflow agent-library-more"><summary><MoreHorizontal size={19} />{t("moreActions")}</summary><div className="agent-library-actions">
            {!official && button("edit", () => edit(definition), item.installation.pinned)}
            {button("fork", () => fork(item))}{button("export", () => { void exported(item); })}
            {button("modelBinding",()=>setBindingItem(item))}
            {button(item.installation.pinned ? "unpin" : "pin", () => act("pin", {id: definition.definition_id, value: !item.installation.pinned}))}
            {button(item.installation.enabled ? "disable" : "enable", () => act("enable", {id: definition.definition_id, value: !item.installation.enabled}))}
            {item.installation.origin === "local" && button("share", () => act("share", {id: definition.definition_id}), !library.data?.account)}
            {button("history", () => { void scopedMutation("history", {id: definition.definition_id}, value => setHistory(value as Package[])); })}
            {button("trial", () => { if (!allowDiscard()) return; const task = window.prompt(t("trialTask")); if (task) onTrial(t("trialPrompt").replace("{id}", definition.definition_id).replace("{task}", task)); }, !item.compatible || !item.installation.enabled)}
            {button("remove", () => { if (window.confirm(t("deleteConfirm"))) act("delete", {id: definition.definition_id}); })}
          </div></details></div>
          <details className="agent-definition-details"><summary>{t("agentDetails")}</summary><code>{definition.definition_id} · {item.package.sha256}</code><pre>{JSON.stringify(definition, null, 2)}</pre></details>
          {official && <p>{t("rsiForkRequired")}</p>}
        </article>;
      }) : <div className="agent-empty-inline">{t("selectAgent")}</div>}</div>
    </section>}</>}
{activeTab === "curated" && <section>
  <h2>{t("curated")}</h2><p>{t("catalogHelp")}</p>
  {button("retryCatalog", () => {void catalog.refetch();}, catalog.isFetching)}
  <div className="agent-library-filters"><label><Search size={18}/><input aria-label={t("search")} placeholder={t("search")} value={search} onChange={event => setSearch(event.target.value)}/></label><select aria-label={t("category")} value={category} onChange={event => setCategory(event.target.value)}><option value="">{t("allCategories")}</option>{categories.map(value => <option key={value} value={value}>{t(value as AgentTextKey) || value}</option>)}</select></div>
  {catalog.data && !catalog.data.items.length && <p>{t("catalogEmpty")}</p>}
  {catalog.data && !!catalogItems.length && !recommendations.length && <p>{t("noMatches")}</p>}
  <div className="agent-curated-grid">{recommendations.map(item => <article className="agent-library-card" key={item.definition_id}>
    <h3>{displayName(item)}</h3><p>{item.discovery ? localized(item.discovery.summary) : item.purpose}</p><p>{t(item.base_role as AgentTextKey)} · {item.version}</p>
    {item.discovery && <p>{t("exampleTask")}: {localized(item.discovery.example)}</p>}
    <details><summary>{t("agentDetails")}</summary><code>{item.definition_id} · {item.sha256}</code></details>
    {items.some(installed => installed.package.sha256 === item.sha256) ? <p>{t("installed")}</p> : button("preview", () => {void scopedMutation("preview", {id: item.definition_id, revision: item.sha256}, value => setPreview(value as Package));})}
  </article>)}</div>
</section>}
    {preview && <section className="agent-library-editor"><h2>{t("newRevision")}</h2><p>{displayName(preview.definition)} · {preview.definition.version}</p><p>{t("agentDetails")}: {preview.definition.tools.join(", ")}</p><details><summary>{t("preview")}</summary><pre>{JSON.stringify(preview.definition, null, 2)}</pre>{items.find(item => item.package.definition.definition_id === preview.definition.definition_id) && <><h3>{t("currentRevision")}</h3><pre>{JSON.stringify(items.find(item => item.package.definition.definition_id === preview.definition.definition_id)?.package.definition, null, 2)}</pre></>}</details>{button("install", () => {void scopedMutation("install", {id: preview.definition.definition_id, revision: preview.sha256}, () => {setPreview(null); setTab("installed"); setSelected(`local/${preview.definition.definition_id}`); setSearch(""); setSource("all");});})}{button("cancel", () => setPreview(null))}</section>}
    {!!history.length && <section><h2>{t("history")}</h2>{history.map(item => <article className="agent-history-row" key={item.sha256}><span>{displayName(item.definition)} · {item.definition.version}</span>{button("rollback", () => act("rollback", {id: item.definition.definition_id, revision: item.sha256}))}<details><summary>{t("agentDetails")}</summary><code>{item.definition.definition_id} · {item.sha256}</code></details></article>)}</section>}
    {result && <details><summary>{t("done")}</summary><pre>{result}</pre></details>}
  </main>;
}
