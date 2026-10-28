import type { Messages } from "./i18n";

export type MarkdownCopyLabels = { copy: string; copied: string; failed: string };

export function markdownCopy(t: Pick<Messages, "copyCode" | "copiedCode" | "copyFailed">): MarkdownCopyLabels {
  return { copy: t.copyCode, copied: t.copiedCode, failed: t.copyFailed };
}

export function queueCopyLabels(copy: string, copied: string, failed: string): MarkdownCopyLabels {
  return { copy, copied, failed };
}
