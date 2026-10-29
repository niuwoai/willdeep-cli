import { describe, expect, it } from "vitest";
import { convertForeignAgent } from "./agentImportConversion";

describe("foreign agent conversion", () => {
  it("converts Hermes persona and model without copying credentials or execution settings", () => {
    const draft = convertForeignAgent("hermes", "model:\n  default: vision-model\n  provider: example\n  api_key: secret-value\nagent:\n  max_turns: 8", "Review images carefully.", "user/vision", "Vision");
    expect(draft.model).toBe("vision-model");
    expect(draft.provider).toBe("example");
    expect(draft.definition.instructions).toBe("Review images carefully.");
    expect(JSON.stringify(draft)).not.toContain("secret-value");
    expect(draft.omitted).toContain("agent");
    expect(draft.omitted).toContain("model.api_key");
  });
  it("converts Multica API agent JSON without inheriting authority", () => {
    const draft = convertForeignAgent("multica", JSON.stringify({name:"Reviewer",instructions:"Review changes",description:"Reviews code",model:"review-model",custom_env:{TOKEN:"secret"},runtime_id:"remote",skills:[{id:"skill"}]}), "", "user/reviewer", "");
    expect(draft.definition.name).toEqual({en:"Reviewer"});
    expect(draft.model).toBe("review-model");
    expect(draft.omitted).toEqual(["custom_env","runtime_id","skills"]);
    expect(JSON.stringify(draft)).not.toContain("secret");
  });
  it.each(["model: first\nmodel: second", "model: &model {default: a}\nother: *model", "[]"])("rejects ambiguous or unsupported YAML", source => {
    expect(() => convertForeignAgent("hermes", source, "Instructions", "user/example", "Example")).toThrow();
  });
});
