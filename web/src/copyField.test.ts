import { describe, expect, it } from "vitest";
import { copyFieldMode, copyFieldFromPre } from "./copyField";
import { createElement } from "react";

describe("copyFieldMode", () => {
  it("recognizes input and textarea fence languages", () => {
    expect(copyFieldMode("language-input")).toBe("input");
    expect(copyFieldMode("hljs language-textarea")).toBe("textarea");
    expect(copyFieldMode("language-sh")).toBeNull();
  });
});

describe("copyFieldFromPre", () => {
  it("extracts text from an input fence", () => {
    const preChild = createElement("code", { className: "language-input" }, "npm run dev");
    const field = copyFieldFromPre(preChild);
    expect(field).toEqual({ mode: "input", text: "npm run dev" });
  });

  it("keeps multiline content visible even when it was labeled input", () => {
    const preChild = createElement("code", { className: "language-input" }, "one\n\n  two\n");
    expect(copyFieldFromPre(preChild)).toEqual({ mode: "textarea", text: "one\n\n  two" });
  });
});
