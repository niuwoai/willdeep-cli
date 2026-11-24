import { describe, expect, it } from "vitest";
import { fileArtifactFromHref, fileArtifactGlyph } from "./fileArtifactLink";

describe("fileArtifactFromHref", () => {
  it("accepts relative artifact paths", () => {
    expect(fileArtifactFromHref("src/App.tsx", "App.tsx")).toMatchObject({ name: "App.tsx", ext: "tsx" });
    expect(fileArtifactFromHref("docs/notes.md", "notes")).toMatchObject({ ext: "md" });
  });

  it("ignores external and extensionless links", () => {
    expect(fileArtifactFromHref("https://example.com/x.js", "x.js")).toBeNull();
    expect(fileArtifactFromHref("README", "README")).toBeNull();
  });
});

describe("fileArtifactGlyph", () => {
  it("maps common extensions", () => {
    expect(fileArtifactGlyph("md")).toBe("MD");
    expect(fileArtifactGlyph("js")).toBe("JS");
  });
});
