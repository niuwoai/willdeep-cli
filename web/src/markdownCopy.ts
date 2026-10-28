import type { Messages } from "./i18n";

export type MarkdownCopyLabels = { copy: string; copied: string; failed: string };

// 同一份 Messages 只生成一次标签对象：Markdown 靠引用相等跳过重复解析。
const labelsByMessages = new WeakMap<object, MarkdownCopyLabels>();

export function markdownCopy(t: Pick<Messages, "copyCode" | "copiedCode" | "copyFailed">): MarkdownCopyLabels {
  const cached = labelsByMessages.get(t);
  if (cached) return cached;
  const labels = { copy: t.copyCode, copied: t.copiedCode, failed: t.copyFailed };
  labelsByMessages.set(t, labels);
  return labels;
}

const queueLabelsByText = new Map<string, MarkdownCopyLabels>();

export function queueCopyLabels(copy: string, copied: string, failed: string): MarkdownCopyLabels {
  const key = `${copy}\u0000${copied}\u0000${failed}`;
  const cached = queueLabelsByText.get(key);
  if (cached) return cached;
  const labels = { copy, copied, failed };
  queueLabelsByText.set(key, labels);
  return labels;
}
