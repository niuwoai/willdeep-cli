import { parseDocument } from "yaml";

export type ForeignFormat = "hermes" | "multica";
export type ImportDraft = { definition: Record<string, unknown>; model: string; provider: string; omitted: string[] };
function object(value: unknown): Record<string, unknown> {
  if (!value || typeof value !== "object" || Array.isArray(value)) throw new Error("invalidImport");
  return value as Record<string, unknown>;
}
function string(value: unknown): string { return typeof value === "string" ? value : ""; }

export function convertForeignAgent(format: ForeignFormat, source: string, soul: string, id: string, name: string): ImportDraft {
  if (new TextEncoder().encode(source + soul).length > 256 * 1024) throw new Error("invalidImport");
  let data: Record<string, unknown>;
  try {
    if (format === "hermes") {
      const document = parseDocument(source, { uniqueKeys: true });
      if (document.errors.length) throw new Error();
      data = object(document.toJS({ maxAliasCount: 0 }));
    } else data = object(JSON.parse(source));
  } catch { throw new Error("invalidImport"); }
  if (!/^user\/[a-z0-9][a-z0-9._/-]*$/.test(id) || id.includes("..") || id.length > 96) throw new Error("invalidImport");
  const instructions = format === "hermes" ? soul : string(data.instructions);
  const displayName = name.trim() || string(data.name);
  if (!instructions.trim() || !displayName.trim()) throw new Error("invalidImport");
  const model = format === "hermes" && typeof data.model === "object" && data.model !== null ? object(data.model) : {};
  const supported = format === "hermes" ? ["model"] : ["name", "description", "instructions", "model"];
  const omitted = Object.keys(data).filter(key => !supported.includes(key));
  if (format === "hermes") omitted.push(...Object.keys(model).filter(key => !["default", "model", "provider"].includes(key)).map(key => `model.${key}`));
  return {
    definition: { schema: "willdeep.agent-definition.v1", definition_id: id, version: "1.0.0", name: { en: displayName }, purpose: string(data.description) || displayName, base_role: "reviewer", instructions, input_modalities: ["text"], tools: ["workspace.read", "repository.inspect"], skills: [], forked_from: null },
    model: format === "hermes" ? string(model.default) || string(model.model) || string(data.model) : string(data.model),
    provider: format === "hermes" ? string(model.provider) : "",
    omitted,
  };
}
