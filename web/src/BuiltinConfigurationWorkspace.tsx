import { Box } from "@chakra-ui/react";
import type { Messages } from "./i18n";
import type { ColorScheme } from "./theme";
import "./BuiltinConfigurationWorkspace.css";

export function BuiltinConfigurationWorkspace({ messages, locale, colorScheme }: {
  messages: Messages;
  locale: string;
  colorScheme: ColorScheme;
}) {
  const src = `/api/settings/config-page?locale=${encodeURIComponent(locale)}&scheme=${colorScheme}`;
  return <Box className="builtin-configuration-workspace">
    <iframe src={src} title={messages.configTitle} />
  </Box>;
}
