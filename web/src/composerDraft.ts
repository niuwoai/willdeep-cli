import type { Reference } from "./useComposerReferences";

export type DraftReference = Omit<Reference, "kind"> & {
  kind: Reference["kind"] | "skill";
  id: string;
  offset: number;
};
export type ComposerDraft = { text: string; references: DraftReference[] };
export const EMPTY_COMPOSER_DRAFT: ComposerDraft = { text: "", references: [] };

/** Move the reference anchors with the surrounding editable text. Offsets are UTF-16, like textarea selections. */
export function editComposerText(draft: ComposerDraft, text: string, caret?: number): ComposerDraft {
  if (draft.text === text) return draft;
  if (!draft.references.length) return { text, references: draft.references };
  if (caret !== undefined) {
    const suffix = text.slice(caret);
    const oldCaret = draft.text.length - suffix.length;
    if (oldCaret >= 0 && draft.text.endsWith(suffix)) {
      let start = 0;
      // Repeated text can make a pure string diff pick a later edit position.
      while (start < caret && start < oldCaret && draft.text[start] === text[start]) start++;
      return replaceComposerRange(draft, start, oldCaret, text.slice(start, caret));
    }
  }
  let start = 0;
  while (start < text.length && start < draft.text.length && text[start] === draft.text[start]) start++;
  let beforeEnd = draft.text.length;
  let afterEnd = text.length;
  while (beforeEnd > start && afterEnd > start && draft.text[beforeEnd - 1] === text[afterEnd - 1]) {
    beforeEnd--;
    afterEnd--;
  }
  return replaceComposerRange(draft, start, beforeEnd, text.slice(start, afterEnd));
}

export function replaceComposerRange(draft: ComposerDraft, start: number, end: number, replacement: string): ComposerDraft {
  const delta = replacement.length - (end - start);
  const text = draft.text.slice(0, start) + replacement + draft.text.slice(end);
  const references = draft.references.map((reference) => {
    if (reference.offset <= start) return reference;
    const offset = reference.offset < end ? start : reference.offset + delta;
    return offset === reference.offset ? reference : { ...reference, offset };
  });
  return { text, references };
}

export function selectDraftReference(draft: ComposerDraft, start: number, end: number, entry: Omit<DraftReference, "offset">): ComposerDraft {
  const next = replaceComposerRange(draft, start, end, "");
  return { ...next, references: [...next.references, { ...entry, offset: start }] };
}

export function backspaceDraftReference(draft: ComposerDraft, start: number, end: number): DraftReference | undefined {
  if (start !== end) return;
  // A reference at the caret takes precedence over the preceding editable character.
  for (let index = draft.references.length - 1; index >= 0; index--) {
    const reference = draft.references[index];
    if (reference.offset === start) return reference;
  }
}

export function serializeComposerDraft(draft: ComposerDraft): string {
  const ordered = [...draft.references].sort((a, b) => a.offset - b.offset);
  let result = "";
  let offset = 0;
  for (const reference of ordered) {
    result += draft.text.slice(offset, reference.offset);
    if (result && !/\s$/u.test(result)) result += " ";
    result += reference.value;
    if (draft.text[reference.offset] && !/\s/u.test(draft.text[reference.offset])) result += " ";
    offset = reference.offset;
  }
  return (result + draft.text.slice(offset)).trim();
}
