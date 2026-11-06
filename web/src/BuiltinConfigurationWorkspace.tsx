import { Box } from "@chakra-ui/react";
import type { Language, Messages } from "./i18n";
import type { ColorScheme } from "./theme";
import type { IdentityView } from "./identityApi";
import { InstanceIdentitySettings } from "./InstanceIdentitySettings";
import { ConfigurationSettings } from "./ConfigurationSettings";
import { reasoningText } from "./reasoningMessages";
import "./BuiltinConfigurationWorkspace.css";

export function BuiltinConfigurationWorkspace({ messages, locale, colorScheme, onIdentityChange }: {
  messages: Messages;
  locale: Language;
  colorScheme: ColorScheme;
  onIdentityChange: (identity: IdentityView) => void;
}) {
  const src = `/api/settings/config-page?locale=${encodeURIComponent(locale)}&scheme=${colorScheme}`;
  return <Box className="builtin-configuration-workspace">
    <InstanceIdentitySettings language={locale} onIdentityChange={onIdentityChange} />
    <details className="builtin-reasoning-settings">
      <summary>{reasoningText(locale).configTitle}</summary>
      <ConfigurationSettings messages={messages} language={locale} reasoningOnly />
    </details>
    <iframe src={src} title={messages.configTitle} />
  </Box>;
}
