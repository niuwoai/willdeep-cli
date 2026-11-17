import { formatter } from "./formatter.ts";
import { audit } from "./audit.ts";

export function formatLabel(value: string): string {
  return `[${value}]`;
}

export function showLabel(value: string): Record<string, string> {
  const { formatLabel: render } = formatter;
  return {
    direct: formatter.formatLabel(value),
    keyed: formatter["formatLabel"](value),
    extracted: render(value),
    local: formatLabel(value),
    literal: "formatLabel",
    audit: audit.formatLabel,
  };
}
