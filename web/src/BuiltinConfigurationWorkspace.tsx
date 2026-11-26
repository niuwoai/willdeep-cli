import { useEffect, useLayoutEffect, useState } from "react";
import { createPortal } from "react-dom";
import { useQueryClient } from "@tanstack/react-query";
import type { Language, Messages } from "./i18n";
import type { ColorScheme } from "./theme";
import type { IdentityView } from "./identityApi";
import { InstanceIdentitySettings } from "./InstanceIdentitySettings";
import { ProviderReasoningSettings } from "./ProviderReasoningSettings";
import { providerSettingsKey } from "./reasoningDraft";
import "./BuiltinConfigurationWorkspace.css";

type Slot = { element: HTMLDivElement; providerId: string | null };

type SettingsProps = {
  messages: Messages;
  locale: Language;
  onIdentityChange: (identity: IdentityView) => void;
};

function SettingsFrame({ src, messages, locale, onIdentityChange }: SettingsProps & { src: string }) {
  const client = useQueryClient();
  const [document, setDocument] = useState<Document | null>(null);
  const [slots, setSlots] = useState<Slot[]>([]);
  const [configPending, setConfigPending] = useState(false);

  useEffect(() => {
    const main = document?.getElementById("main");
    if (!document || !main) return;
    const mounted = new Map<string, Slot>();
    function syncSlots() {
      if (!main || !document) return;
      const seen = new Set<string>();
      let changed = false;
      for (const placeholder of main.querySelectorAll<HTMLElement>("[data-builtin-setting]")) {
        const providerId = placeholder.dataset.provider ?? null;
        const key = providerId === null ? "identity" : `provider:${providerId}`;
        if (seen.has(key)) continue;
        seen.add(key);
        let slot = mounted.get(key);
        if (!slot) {
          slot = { element: document.createElement("div"), providerId };
          mounted.set(key, slot);
          changed = true;
        }
        if (slot.element.parentElement !== placeholder) {
          placeholder.append(slot.element);
          changed = true;
        }
      }
      for (const key of mounted.keys()) {
        if (!seen.has(key)) { mounted.delete(key); changed = true; }
      }
      if (changed) setSlots([...mounted.values()]);
      setConfigPending(document.documentElement.hasAttribute("data-config-pending"));
    }
    const observer = new MutationObserver(syncSlots);
    observer.observe(main, { childList: true, subtree: true });
    observer.observe(document.documentElement, { attributes: true, attributeFilter: ["data-config-pending"] });
    const refreshProviders = () => void client.invalidateQueries({ queryKey: providerSettingsKey });
    document.addEventListener("willdeep:config-loaded", refreshProviders);
    syncSlots();
    return () => {
      observer.disconnect();
      document.removeEventListener("willdeep:config-loaded", refreshProviders);
    };
  }, [client, document]);

  useLayoutEffect(() => {
    if (slots.length) document?.dispatchEvent(new Event("willdeep:setting-mounted"));
  }, [document, slots]);

  return <>
    <iframe src={src} title={messages.configTitle} onLoad={event => {
      setSlots([]);
      setDocument(event.currentTarget.contentDocument);
    }} />
    {document && slots.map(({ element, providerId }) => createPortal(
      providerId === null ? <InstanceIdentitySettings language={locale} onIdentityChange={onIdentityChange} />
        : <ProviderReasoningSettings providerId={providerId} language={locale} messages={messages} document={document} configPending={configPending} />,
      element,
      providerId === null ? "identity" : `provider:${providerId}`,
    ))}
  </>;
}

export function BuiltinConfigurationWorkspace({ colorScheme, ...props }: SettingsProps & { colorScheme: ColorScheme }) {
  const src = `/api/settings/config-page?locale=${encodeURIComponent(props.locale)}&scheme=${colorScheme}`;
  return <div className="builtin-configuration-workspace"><SettingsFrame key={src} src={src} {...props} /></div>;
}
