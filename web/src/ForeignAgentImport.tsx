import { useState } from "react";
import type { Language } from "./i18n";
import { agentText } from "./agentLibraryMessages";
import { convertForeignAgent, type ForeignFormat, type ImportDraft } from "./agentImportConversion";
import { AgentModelBinding, type LocalAgentBinding } from "./AgentModelBinding";
import { agentLibraryCall } from "./agentLibraryModel";

export function ForeignAgentImport({ language, onDone }: { language: Language; onDone: () => void }) {
  const t = (key: Parameters<typeof agentText>[0]) => agentText(key, language);
  const [format, setFormat] = useState<ForeignFormat>("hermes");
  const [source, setSource] = useState("");
  const [soul, setSoul] = useState("");
  const [name, setName] = useState("");
  const [id, setId] = useState("user/imported-agent");
  const [draft, setDraft] = useState<ImportDraft | null>(null);
  const [error, setError] = useState("");
  const [pending, setPending] = useState(false);
  const read = async (file: File | undefined, apply: (text: string) => void) => {
    if (!file) return;
    try { if (file.size > 256 * 1024) throw new Error(); apply(await file.text()); setDraft(null); }
    catch { setError(t("invalidImport")); }
  };
  const preview = () => {
    setError("");
    try { setDraft(convertForeignAgent(format, source, soul, id, name)); }
    catch { setError(t("invalidImport")); }
  };
  const save = async (binding: LocalAgentBinding) => {
    if (!draft) return;
    setPending(true); setError("");
    try {
      await agentLibraryCall("import", {package_text: JSON.stringify(draft.definition), new_binding: binding});
      setDraft(null); onDone();
    } catch { setError(t("importSaveFailed")); }
    finally { setPending(false); }
  };
  return <details className="agent-library-editor"><summary>{t("foreignImport")}</summary>
    <p>{t("foreignImportHelp")}</p>
    <fieldset disabled={pending}>
      <label>{t("importFormat")}<select value={format} onChange={event => {setFormat(event.target.value as ForeignFormat); setDraft(null);}}><option value="hermes">{t("hermesFormat")}</option><option value="multica">{t("multicaFormat")}</option></select></label>
      <label>{t("id")}<input value={id} onChange={event => {setId(event.target.value); setDraft(null);}} /></label>
      <label>{t("name")}<input value={name} onChange={event => {setName(event.target.value); setDraft(null);}} /></label>
      <label>{t(format === "hermes" ? "hermesConfig" : "multicaConfig")}<input type="file" accept={format === "hermes" ? ".yaml,.yml" : ".json"} onChange={event => void read(event.target.files?.[0], setSource)} /></label>
      {format === "hermes" && <label>{t("hermesSoul")}<input type="file" accept=".md" onChange={event => void read(event.target.files?.[0], setSoul)} /></label>}
      <button type="button" onClick={preview}>{t("preview")}</button>
    </fieldset>
    {error && <p role="alert">{error}</p>}
    {draft && <><pre>{JSON.stringify(draft.definition, null, 2)}</pre><p>{t("importOmitted")}: {draft.omitted.join(", ")}</p><p>{t("importProviderHint")}: {draft.provider}</p>
      <AgentModelBinding key={`${id}/${draft.model}`} language={language} binding={{provider_profile: null, model: draft.model || null}} pending={pending} onSave={binding => void save(binding)} onCancel={() => {setDraft(null);onDone();}} />
    </>}
  </details>;
}
