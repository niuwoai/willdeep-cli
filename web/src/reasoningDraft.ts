import { createRequestId } from "./requestId";
import type { reasoningText } from "./reasoningMessages";

type ReasoningValue = string | number | boolean;
export type ReasoningProfile = { default_level: string | null; parameter: string | null; levels: Record<string, ReasoningValue> };
export type ReasoningEntry = { id: string; level: string; kind: "string" | "number" | "boolean"; value: string };
export type ReasoningDraft = { default_level: string; parameter: string; levels: ReasoningEntry[] };

export function reasoningDraft(profile?: ReasoningProfile | null): ReasoningDraft {
  const levels = Object.entries(profile?.levels ?? {}).map(([level, value]): ReasoningEntry => ({
    id: createRequestId(), level,
    kind: typeof value === "string" ? "string" : typeof value === "number" ? "number" : "boolean",
    value: String(value),
  }));
  return { default_level: profile?.default_level ?? "", parameter: profile?.parameter ?? "", levels };
}

export function reasoningProfile(draft: ReasoningDraft, t: ReturnType<typeof reasoningText>): ReasoningProfile | null {
  const entries = draft.levels.filter((entry) => entry.level.trim() || entry.value.trim());
  const defaultLevel = draft.default_level.trim();
  const parameter = draft.parameter.trim();
  if (!entries.length && !defaultLevel && !parameter) return null;
  const levels: Record<string, ReasoningValue> = Object.create(null);
  for (const entry of entries) {
    const level = entry.level.trim();
    if (!level) throw new Error(t.invalidLevel);
    if (Object.hasOwn(levels, level)) throw new Error(t.duplicateLevel);
    if (entry.kind === "number") {
      const value = Number(entry.value);
      if (!entry.value.trim() || !Number.isFinite(value)) throw new Error(t.invalidNumber);
      levels[level] = value;
    } else {
      levels[level] = entry.kind === "boolean" ? entry.value === "true" : entry.value;
    }
  }
  if (Object.keys(levels).length && (!defaultLevel || !Object.hasOwn(levels, defaultLevel))) throw new Error(t.defaultMissing);
  return { default_level: defaultLevel || null, parameter: parameter || null, levels };
}

export const providerSettingsKey = ["builtin-provider-settings"] as const;
