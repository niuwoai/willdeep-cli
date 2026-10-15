import { useState } from "react";
import { agentText } from "./agentLibraryMessages";
import { nextAgentVersion } from "./agentLibraryModel";
import type { Language } from "./i18n";

export type SyncConflict = {definition_id: string; expected_local_revision: string; expected_remote_etag: string; remote_revision: string; remote_deleted: boolean; local_version: string; remote_version: string; pinned: boolean};
type Choice = "keep_local" | "use_cloud";
export type ResolveParams = {id: string; choice: Choice; expected_local_revision: string; expected_remote_etag: string; new_version?: string};

export function AgentSyncConflicts({conflicts, language, pending, onResolve}: {conflicts: SyncConflict[]; language: Language; pending: boolean; onResolve: (params: ResolveParams) => void}) {
  const t = (key: Parameters<typeof agentText>[0]) => agentText(key,language);
  return <section aria-label={t("syncConflicts")}>
    <h2>{t("syncConflicts")}</h2><p>{t("resolveHelp")}</p>
    {conflicts.map(conflict => <ConflictCard key={`${conflict.definition_id}/${conflict.expected_local_revision}/${conflict.expected_remote_etag}`} conflict={conflict} language={language} pending={pending} onResolve={onResolve} />)}
  </section>;
}

function ConflictCard({conflict,language,pending,onResolve}: {conflict: SyncConflict; language: Language; pending: boolean; onResolve: (params: ResolveParams) => void}) {
  const t = (key: Parameters<typeof agentText>[0]) => agentText(key,language);
  const sameVersionFork = conflict.local_version === conflict.remote_version && conflict.expected_local_revision !== conflict.remote_revision;
  const [version,setVersion] = useState(sameVersionFork ? nextAgentVersion(conflict.remote_version) : "");
  const resolve = (choice: Choice) => onResolve({id:conflict.definition_id,choice,expected_local_revision:conflict.expected_local_revision,expected_remote_etag:conflict.expected_remote_etag,...(choice === "keep_local" && version.trim() ? {new_version:version.trim()} : {})});
  return <article className="agent-library-card">
    <h3>{conflict.definition_id}</h3><p>{t("currentRevision")}: {conflict.local_version} · {conflict.remote_deleted ? t("cloudDeleted") : `${t("cloudRevision")}: ${conflict.remote_version}`}</p>
    {conflict.pinned && <p>{t("resolvePinned")}</p>}
    <label>{t("resolveVersion")}<input value={version} disabled={pending} placeholder={nextAgentVersion(conflict.remote_version)} onChange={event=>setVersion(event.target.value)} /></label>
    <p>{t("resolveVersionHelp")}</p>
    <div className="agent-library-actions"><button type="button" disabled={pending || (sameVersionFork && !version.trim())} onClick={()=>resolve("keep_local")}>{t("keepLocal")}</button>
    <button type="button" disabled={pending} onClick={()=>resolve("use_cloud")}>{t("useCloud")}</button></div>
    <details><summary>{t("resolveDetails")}</summary><p>{conflict.expected_local_revision}</p><p>{conflict.remote_revision}</p><p>{conflict.expected_remote_etag}</p></details>
  </article>;
}
