import { describe, expect, it } from "vitest";
import { messages } from "./i18n";
import { runtimeFailureMessage, runtimeFailureReference } from "./runtimeErrors";

describe("Runtime failure projection", () => {
  it("uses the safe server detail for both stream and historical failure objects", () => {
    const failure = { message: "turn.failed", session_id: "session", task_id: "task", reason: "unclassified", failure_domain: "provider", failure_detail: "The selected model does not support tools" };
    for (const t of Object.values(messages)) {
      const text = runtimeFailureMessage(failure, t);
      expect(text).toContain("The selected model does not support tools");
      expect(text).toContain(t.failureProvider);
      expect(text).not.toContain(t.turnFailureUnclassified);
    }
    expect(runtimeFailureReference(failure, "session")).toEqual({ sessionId: "session", taskId: "task" });
  });

  it("does not expose raw message strings from old servers or unknown failure classes", () => {
    const t = messages["zh-CN"];
    const text = runtimeFailureMessage({ message: "private raw error /Users/private/file", reason: "private unknown classification", failure_domain: "private internal domain" }, t);
    expect(text).toContain(t.turnFailureUnknown);
    expect(text).not.toContain("private");
    expect(runtimeFailureMessage({ message: "turn.failed", reason: "quota", failure_detail: "  " }, t)).toContain(t.turnFailureQuota);
  });

  it("keeps cancellation neutral and uses specific interruption causes when available", () => {
    const t = messages.en;
    expect(runtimeFailureMessage({ message: "turn.cancelled", failure_detail: "ignored" }, t)).toBe(t.turnFailureCancelled);
    expect(runtimeFailureMessage({ message: "turn.interrupted", failure_detail: "Stream closed before completion" }, t)).toContain("Stream closed before completion");
    expect(runtimeFailureMessage({ message: "turn.interrupted" }, t)).toContain(t.turnFailureInterrupted);
  });

  it("never builds a diagnostic target for another session or missing task", () => {
    expect(runtimeFailureReference({ session_id: "another", task_id: "task", message: "turn.failed" }, "current")).toBeNull();
    expect(runtimeFailureReference({ session_id: "current", message: "turn.failed" }, "current")).toBeNull();
    expect(runtimeFailureReference({ session_id: "current", task_id: "task", message: "turn.cancelled" }, "current")).toBeNull();
  });
});
