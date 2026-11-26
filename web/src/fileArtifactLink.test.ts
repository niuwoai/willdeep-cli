import { describe, expect, it } from "vitest";
import { fileArtifactFromHref, fileArtifactGlyph, isBrowserLink } from "./fileArtifactLink";

describe("fileArtifactFromHref", () => {
  it.each([
    ["src/App.tsx", "src/App.tsx", "App.tsx"],
    ["./docs/note%20file.md#L2", "./docs/note file.md", "note file.md"],
    ["file:///work/notes.md", "/work/notes.md", "notes.md"],
    ["C:/work/main.js", "C:/work/main.js", "main.js"],
  ])("recognizes %s as a workspace path", (href, path, name) => {
    expect(fileArtifactFromHref(href)).toMatchObject({ name, path });
  });

  it.each([
    "https://example.com/x.js", "README", "file://remote/x.js", "//remote/x.js",
    "../private.md", "docs/%2e%2e/private.md", "docs%5cprivate.md", "docs/%00file.md", "%XXXX.md",
  ])("ignores non-artifact or unsafe href %s", (href) => {
    expect(fileArtifactFromHref(href)).toBeNull();
  });
});

it("recognizes HTTP(S) links without user information", () => {
  expect(isBrowserLink("http://localhost:3000/")).toBe(true);
  expect(isBrowserLink("https://example.org/report.md?author=a@b")).toBe(true);
  expect(isBrowserLink("https://user:example@example.org/")).toBe(false);
  expect(isBrowserLink("javascript:alert(1)")).toBe(false);
});

describe("fileArtifactGlyph", () => {
  it("maps common extensions", () => {
    expect(fileArtifactGlyph("md")).toBe("MD");
    expect(fileArtifactGlyph("js")).toBe("JS");
  });
});
