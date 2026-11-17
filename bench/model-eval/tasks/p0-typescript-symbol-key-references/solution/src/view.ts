import { formatter } from "./formatter.ts";
import { audit } from "./audit.ts";

export function formatLabel(value: string): string {
  return `[${value}]`;
}

export function showLabel(value: string): Record<string, string> {
  const { renderLabel: render } = formatter;
  return {
    direct: formatter.renderLabel(value),
    keyed: formatter["renderLabel"](value),
    extracted: render(value),
    local: formatLabel(value),
    literal: "formatLabel",
    audit: audit.formatLabel,
  };
}
