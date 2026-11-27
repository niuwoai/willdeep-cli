import { describe, expect, it } from "vitest";
import { backspaceDraftReference, editComposerText, selectDraftReference, serializeComposerDraft } from "./composerDraft";
import { referenceQuery } from "./useComposerReferences";

const file = { id: "file", kind: "file", name: "src/中文 file.rs", value: '@file:"src/中文 file.rs"' } as const;

describe("composer references", () => {
  it("supports Unicode paths and cursor position without matching email addresses", () => {
    expect(referenceQuery("user@example.com", 16)).toBeUndefined();
    expect(referenceQuery("看 @src/中文.rs 后文", 12)?.query).toBe("src/中文.rs");
    expect(referenceQuery("@file:\"a b\" ", 12)).toBeUndefined();
  });
  it("selects an entire query and preserves the quoted token between surrounding text", () => {
    const text = "看 @source 后文";
    const query = referenceQuery(text, 6)!;
    const draft = selectDraftReference({ text, references: [] }, query.start, query.end, file);
    expect(draft.text).toBe("看  后文");
    expect(serializeComposerDraft(draft)).toBe('看 @file:"src/中文 file.rs" 后文');
    expect(backspaceDraftReference(draft, 2, 2)?.id).toBe("file");
    expect(backspaceDraftReference(draft, 2, 3)).toBeUndefined();
  });

  it("uses the caret to anchor references when editing repeated text", () => {
    const draft = selectDraftReference({ text: "aa@fileaa", references: [] }, 2, 7, file);
    const inserted = editComposerText(draft, "aaaaa", 2);
    expect(inserted.references[0].offset).toBe(3);
    const deleted = editComposerText(inserted, "aaaa", 1);
    expect(deleted.references[0].offset).toBe(2);
  });

  it("keeps references anchored when text before them shifts", () => {
    const draft = selectDraftReference({ text: "看看 @file 然后比较", references: [] }, 3, 8, file);
    const next = editComposerText(draft, "请先看看  然后比较");
    expect(next.references[0].offset).toBe(5);
    expect(serializeComposerDraft(next)).toBe('请先看看 @file:"src/中文 file.rs" 然后比较');
  });
});
