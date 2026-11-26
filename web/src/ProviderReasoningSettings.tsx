import { useId, useMemo, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import type { ProviderSettingsSnapshot } from "./ConfigurationSettings";
import type { Language, Messages } from "./i18n";
import { identityText } from "./identityMessages";
import { ReasoningFields } from "./ReasoningFields";
import { providerSettingsKey, reasoningDraft, reasoningProfile, type ReasoningDraft } from "./reasoningDraft";
import { reasoningText } from "./reasoningMessages";
const endpoint = "/api/settings/providers";

async function providerResponse(response: Response, staleMessage?: string): Promise<ProviderSettingsSnapshot> {
  if (response.status === 409 && staleMessage) throw new Error(staleMessage);
  const value = await response.json() as ProviderSettingsSnapshot & { error?: string };
  if (!response.ok) throw new Error(value.error || `HTTP ${response.status}`);
  return value;
}

export function ProviderReasoningSettings({ providerId, language, messages, document, configPending }: {
  providerId: string; language: Language; messages: Messages; document: Document; configPending: boolean;
}) {
  const t = reasoningText(language);
  const titleId = useId();
  const client = useQueryClient();
  const [expanded, setExpanded] = useState(false);
  const [edit, setEdit] = useState<{ revision: string; draft: ReasoningDraft } | null>(null);
  const [saved, setSaved] = useState(false);
  const snapshot = useQuery({
    queryKey: providerSettingsKey,
    queryFn: async ({ signal }) => providerResponse(await fetch(endpoint, { signal })),
    enabled: expanded, retry: false, refetchOnWindowFocus: false,
  });
  const provider = snapshot.data?.providers.find(item => item.id === providerId);
  const original = useMemo(() => reasoningDraft(provider?.reasoning), [provider?.reasoning]);
  const save = useMutation({
    mutationFn: async () => {
      if (!provider || !edit) return;
      const reasoning = reasoningProfile(edit.draft, t);
      await client.cancelQueries({ queryKey: providerSettingsKey });
      return providerResponse(await fetch(endpoint, {
        method: "PUT", headers: { "content-type": "application/json" },
        body: JSON.stringify({
          id: provider.id, provider: provider.provider, api: provider.api, api_base: provider.api_base,
          model: provider.model, api_key: "", set_default: false, revision: edit.revision, reasoning_only: true, reasoning,
        }),
      }), messages.configStale);
    },
    onSuccess: next => {
      if (!next) return;
      client.setQueryData(providerSettingsKey, next);
      setEdit(null);
      setSaved(true);
      document.dispatchEvent(new Event("willdeep:reasoning-saved"));
    },
  });
  const stale = !!edit && !!snapshot.data && edit.revision !== snapshot.data.revision;
  return <div className="row builtin-reasoning-row" data-search={`${providerId} ${t.configTitle} ${t.defaultLevel} ${t.parameter} reasoning`.toLowerCase()}>
    <details className="builtin-reasoning" open={expanded} onToggle={event => setExpanded(event.currentTarget.open)}>
      <summary className="row-label" id={titleId}>{t.configTitle}</summary>
      {expanded && <form aria-labelledby={titleId} onSubmit={event => {
        event.preventDefault();
        if (provider && edit && !stale && !save.isPending && !snapshot.isFetching && !document.documentElement.hasAttribute("data-config-pending")) save.mutate();
      }}>
        <p className="sub">{t.configHint}</p>
        {snapshot.isPending ? <p role="status">{t.loading}</p> : snapshot.isError || !provider ? <div role="alert">
          {messages.configLoadFailed} {snapshot.error?.message}
          <button type="button" className="btn btn-sm" onClick={() => void snapshot.refetch()}>{identityText(language).retry}</button>
        </div> : <>
          <ReasoningFields draft={edit?.draft ?? original} labels={t} disabled={save.isPending} onChange={draft => {
            setEdit({ revision: edit?.revision ?? snapshot.data.revision, draft });
            setSaved(false);
          }} />
          <div className="row-control">
            <button type="submit" className="btn btn-sm" disabled={!edit || stale || save.isPending || snapshot.isFetching || configPending}>{save.isPending ? messages.configSaving : t.saveMappings}</button>
            {(stale || save.isError) && <button type="button" className="btn btn-sm" disabled={snapshot.isFetching} onClick={() => {
              setEdit(null);
              save.reset();
              void snapshot.refetch();
            }}>{t.reloadMappings}</button>}
          </div>
          {configPending && edit && <p className="sub" role="status">{t.finishConfigEdits}</p>}
          {(stale || save.isError) && <p className="builtin-setting-error" role="alert">{stale ? messages.configStale : save.error?.message}</p>}
          {saved && <p role="status">{messages.configSaved}</p>}
        </>}
      </form>}
    </details>
  </div>;
}
