import { useEffect, useState, type FormEvent } from "react";
import { Box, Button, Flex, Heading, Input, NativeSelect, Text } from "@chakra-ui/react";
import { ModelRoutingSettings } from "./ModelRoutingSettings";
import type { Language, Messages } from "./i18n";
import { ReasoningFields } from "./ReasoningFields";
import { reasoningDraft, reasoningProfile, type ReasoningDraft, type ReasoningProfile } from "./reasoningDraft";
import { reasoningText } from "./reasoningMessages";

type Provider = { id: string; provider: string; api: string; api_base: string; model: string; api_key_configured: boolean; reasoning?: ReasoningProfile | null };
export type ProviderSettingsSnapshot = { revision: string; default_provider: string | null; providers: Provider[]; config_path: string };
type Draft = { id: string; provider: string; api: string; api_base: string; api_key: string; model: string; set_default: boolean; reasoning: ReasoningDraft };

const emptyDraft = (): Draft => ({ id: "default", provider: "openai-compatible", api: "chat-completions", api_base: "https://api.openai.com/v1", api_key: "", model: "", set_default: true, reasoning: reasoningDraft() });

const control = { color: "var(--text)", bg: "var(--bg-raised)", borderColor: "var(--border-strong)" };

export function ConfigurationSettings({ messages: t, language, compact = false, reasoningOnly = false, onDone }: { messages: Messages; language: Language; compact?: boolean; reasoningOnly?: boolean; onDone?: () => void }) {
  const reasoning = reasoningText(language);
  const [snapshot, setProviderSettingsSnapshot] = useState<ProviderSettingsSnapshot | null>(null);
  const [draft, setDraft] = useState<Draft>(emptyDraft);
  const [loading, setLoading] = useState(false);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState("");
  const [saved, setSaved] = useState(false);
  const [routingOpen, setRoutingOpen] = useState(false);

  useEffect(() => {
    let cancelled = false;
    async function load() {
      setLoading(true); setError("");
      try {
        const response = await fetch("/api/settings/providers");
        const value = await response.json() as ProviderSettingsSnapshot & { error?: string };
        if (!response.ok) throw new Error(value.error || response.statusText);
        if (cancelled) return;
        setProviderSettingsSnapshot(value);
        if (!value.providers.length) setDraft(emptyDraft());
        else {
          const provider = value.providers.find(item => item.id === value.default_provider) || value.providers[0];
          setDraft({ id: provider.id, provider: provider.provider, api: provider.api, api_base: provider.api_base, api_key: "", model: provider.model, set_default: provider.id === value.default_provider, reasoning: reasoningDraft(provider.reasoning) });
        }
      } catch (reason) {
        if (!cancelled) setError(`${t.configLoadFailed}: ${reason instanceof Error ? reason.message : String(reason)}`);
      } finally { if (!cancelled) setLoading(false); }
    }
    void load();
    return () => { cancelled = true; };
  }, [t.configLoadFailed]);

  async function save(event: FormEvent) {
    event.preventDefault();
    if (!snapshot) return;
    setSaving(true); setError(""); setSaved(false);
    try {
      const response = await fetch("/api/settings/providers", {
        method: "PUT", headers: { "content-type": "application/json" },
        body: JSON.stringify({
          id: draft.id, provider: draft.provider, api: draft.api, api_base: draft.api_base,
          api_key: draft.api_key, model: draft.model, set_default: draft.set_default,
          revision: snapshot.revision,
          ...(reasoningOnly ? { reasoning_only: true } : {}),
          ...((!compact || reasoningOnly) ? { reasoning: reasoningProfile(draft.reasoning, reasoning) } : {}),
        }),
      });
      const value = await response.json() as ProviderSettingsSnapshot & { error?: string };
      if (!response.ok) {
        if (response.status === 409) throw new Error(t.configStale);
        throw new Error(value.error || response.statusText);
      }
      setProviderSettingsSnapshot(value);
      setDraft({ ...draft, api_key: "", reasoning: reasoningDraft(value.providers.find((provider) => provider.id === draft.id)?.reasoning) });
      setSaved(true);
      if (onDone && value.providers.some(provider => provider.id === value.default_provider && provider.model.trim())) onDone();
    } catch (reason) {
      setError(`${t.configSaveFailed}: ${reason instanceof Error ? reason.message : String(reason)}`);
    } finally { setSaving(false); }
  }

  function selectProvider(id: string) {
    const provider = snapshot?.providers.find(item => item.id === id);
    if (!provider) return;
    setDraft({ id: provider.id, provider: provider.provider, api: provider.api, api_base: provider.api_base, api_key: "", model: provider.model, set_default: id === snapshot?.default_provider, reasoning: reasoningDraft(provider.reasoning) });
    setSaved(false);
  }

  return <Box className={`configuration-settings${compact ? " configuration-quick-setup" : ""}${reasoningOnly ? " configuration-reasoning-only" : ""}`}>
    {!compact && !reasoningOnly && <Flex justify="space-between" align="center" mb="5" wrap="wrap" gap="3"><Box><Heading size="md">{t.configTitle}</Heading><Text fontSize="sm" color="var(--text-dim)" mt="1">{snapshot?.config_path}</Text></Box><Button variant="outline" onClick={() => setRoutingOpen(true)}>{t.configAdvanced}</Button></Flex>}
    {compact && <Box mb="3"><Heading size="md">{t.setupTitle}</Heading><Text color="var(--text-dim)" mt="1">{t.setupConfigure}</Text><Text fontSize="sm" color="var(--text-faint)" mt="1">{t.setupSteps}</Text></Box>}
    {error && <Text role="alert" color="var(--danger-text)" mb="3">{error}</Text>}
    {loading && <Text color="var(--text-dim)" mb="3">{t.routingLoading}</Text>}
    {snapshot?.providers.length ? <Flex gap="2" wrap="wrap" mb="4" aria-label={t.configTitle}>
      {snapshot.providers.map(provider => <Button key={provider.id} size="sm" variant={provider.id === draft.id ? "solid" : "outline"} onClick={() => selectProvider(provider.id)}>{provider.id}{provider.id === snapshot.default_provider ? ` · ${t.configSetDefault}` : ""}</Button>)}
      {!reasoningOnly && <Button size="sm" variant="ghost" onClick={() => { setDraft({ ...emptyDraft(), id: `provider-${snapshot.providers.length + 1}` }); setSaved(false); }}>{t.configAddProvider}</Button>}
    </Flex> : !compact && snapshot && <Text color="var(--text-dim)" mb="3">{t.configNoProviders}</Text>}
    <form onSubmit={(event) => void save(event)} className="configuration-form">
      {!reasoningOnly && <>
      <label><span>{t.configProviderId}</span><Input required maxLength={64} pattern="[A-Za-z0-9_-]+" value={draft.id} onChange={event => { setDraft({ ...draft, id: event.target.value }); setSaved(false); }} {...control} /></label>
      <label><span>{t.configProviderType}</span><NativeSelect.Root><NativeSelect.Field value={draft.provider} onChange={event => { const provider = event.target.value; setDraft({ ...draft, provider, api: provider === "anthropic" ? "anthropic-messages" : "chat-completions", api_base: provider === "anthropic" ? "https://api.anthropic.com" : draft.api_base }); setSaved(false); }} {...control}>
        <option value="openai-compatible">{t.configApiCompatible}</option><option value="anthropic">{t.configAnthropic}</option><option value="some-im">{t.configSomeIm}</option>
      </NativeSelect.Field><NativeSelect.Indicator /></NativeSelect.Root></label>
      <label><span>{t.configProtocol}</span><NativeSelect.Root><NativeSelect.Field value={draft.api} onChange={event => { setDraft({ ...draft, api: event.target.value }); setSaved(false); }} {...control}>
        <option value="chat-completions">{t.configChatCompletions}</option><option value="anthropic-messages">{t.configAnthropicMessages}</option><option value="responses">{t.configResponses}</option>
      </NativeSelect.Field><NativeSelect.Indicator /></NativeSelect.Root></label>
      <label><span>{t.configApiBase}</span><Input required type="url" value={draft.api_base} onChange={event => { setDraft({ ...draft, api_base: event.target.value }); setSaved(false); }} {...control} /></label>
      <label><span>{t.configApiKey}{draft.api_key === "" && snapshot?.providers.find(provider => provider.id === draft.id)?.api_key_configured ? ` · ${t.configKeySaved}` : ""}</span><Input type="password" autoComplete="new-password" value={draft.api_key} placeholder={t.configApiKeyPlaceholder} onChange={event => { setDraft({ ...draft, api_key: event.target.value }); setSaved(false); }} {...control} /></label>
      <label><span>{t.configModel}</span><Input required value={draft.model} onChange={event => { setDraft({ ...draft, model: event.target.value }); setSaved(false); }} {...control} /></label>
      </>}
      {(!compact || reasoningOnly) && <section className="configuration-reasoning">
        <Heading size="sm">{reasoning.configTitle}</Heading>
        <Text fontSize="sm" color="var(--text-dim)">{reasoning.configHint}</Text>
        <ReasoningFields draft={draft.reasoning} labels={reasoning} disabled={saving} onChange={next => {
          setDraft({ ...draft, reasoning: next });
          setSaved(false);
        }} />
      </section>}
      {!reasoningOnly && <label className="configuration-default"><input type="checkbox" checked={draft.set_default} onChange={event => { setDraft({ ...draft, set_default: event.target.checked }); setSaved(false); }} />{t.configSetDefault}</label>}
      <Flex gap="3" align="center" mt="3"><Button type="submit" disabled={saving || loading || !snapshot || (reasoningOnly && !snapshot.providers.length)}>{saving ? t.configSaving : compact ? t.setupDone : t.configSave}</Button>{saved && <Text role="status" color="var(--success)">{t.configSaved}</Text>}</Flex>
    </form>
    {!reasoningOnly && <ModelRoutingSettings messages={t} open={routingOpen} onOpenChange={setRoutingOpen} />}
  </Box>;
}
