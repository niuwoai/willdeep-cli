import type { ReactElement, ReactNode } from "react";
import { Children, isValidElement } from "react";

export type CopyFieldMode = "input" | "textarea";

const INPUT_LANGS = new Set(["input", "in"]);
const TEXTAREA_LANGS = new Set(["textarea", "text", "output"]);

export function copyFieldMode(className: string | undefined): CopyFieldMode | null {
  if (!className) return null;
  const lang = [...className.matchAll(/language-([\w-]+)/g)].map((match) => match[1].toLowerCase()).pop();
  if (!lang) return null;
  if (INPUT_LANGS.has(lang)) return "input";
  if (TEXTAREA_LANGS.has(lang)) return "textarea";
  return null;
}

export function markdownNodeText(node: ReactNode): string {
  if (node === null || node === undefined || typeof node === "boolean") return "";
  if (typeof node === "string" || typeof node === "number") return String(node);
  if (Array.isArray(node)) return node.map(markdownNodeText).join("");
  if (isValidElement(node)) return markdownNodeText((node as ReactElement<{ children?: ReactNode }>).props.children);
  return "";
}

export function copyFieldFromPre(children: ReactNode): { mode: CopyFieldMode; text: string } | null {
  const child = Children.toArray(children).find(isValidElement);
  if (!child || !isValidElement(child)) return null;
  const props = child.props as { className?: string; children?: ReactNode };
  const mode = copyFieldMode(props.className);
  if (!mode) return null;
  const text = markdownNodeText(props.children).replace(/\n$/, "");
  if (!text.trim()) return null;
  return { mode: mode === "input" && text.includes("\n") ? "textarea" : mode, text };
}
