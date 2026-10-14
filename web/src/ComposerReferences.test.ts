import { describe, expect, it } from "vitest";
import { insertReference, referenceQuery } from "./useComposerReferences";

describe("composer references", () => {
  it("supports Unicode paths and cursor position without matching email addresses", () => {
    expect(referenceQuery("user@example.com", 16)).toBeUndefined();
    expect(referenceQuery("看 @src/中文.rs 后文", 12)?.query).toBe("src/中文.rs");
    expect(referenceQuery("@file:\"a b\" ", 12)).toBeUndefined();
  });
  it("inserts a quoted file while preserving text after the cursor", () => {
    const text = "看 @src 后文";
    const query = referenceQuery(text, 6)!;
    expect(insertReference(text, query, '@file:"src/中文 file.rs"')).toBe('看 @file:"src/中文 file.rs"  后文');
  });
});
