import { describe, expect, it } from "vitest";
import { enqueueButtonState, queueText, runningInputFeedback } from "./queueMessages";
import type { RuntimeActivity } from "./RuntimeSidebar";

it("progress feedback reports missing evidence and never borrows another session's tools or gate", () => {
  const activity: RuntimeActivity = {
    tools: [{ id: "tool", session_id: "other", turn_id: null, task_id: "task", agent_id: "agent", name: "private_tool", status: "running", started_at_ms: 1, completed_at_ms: null }],
    tasks: [{ id: "task", session_id: "other", turn_id: null, agent_id: "agent", status: "waiting_approval", profile: null, elapsed_seconds: 1, exit_code: null, failure_domain: null }],
    gates: [{ kind: "approval", id: "gate", task_id: "task", description: "other session's operation", always_allow_available: false }],
    artifacts: [], agents: [], attention_count: 1,
  };
  for (const language of ["zh-CN", "en", "ja"] as const) {
    expect(runningInputFeedback(activity, "current", language)).toEqual({ gateId: undefined, text: queueText("noProgress", language) });
  }
});

describe("空闲时入队", () => {
  it("输入框有内容时按钮出现，这一轮是否在跑不参与判断", () => {
    expect(enqueueButtonState({ hasDraft: true, hasSession: true })).toEqual({ visible: true, enabled: true });
  });

  it("输入框什么都没有时不出现", () => {
    expect(enqueueButtonState({ hasDraft: false, hasSession: true }).visible).toBe(false);
    expect(enqueueButtonState({ hasDraft: false, hasSession: false }).visible).toBe(false);
  });

  it("还没有会话时看得见但点不了", () => {
    expect(enqueueButtonState({ hasDraft: true, hasSession: false })).toEqual({ visible: true, enabled: false });
  });

  it("三种语言都说明了为什么不能入队", () => {
    for (const language of ["zh-CN", "en", "ja"] as const) {
      expect(queueText("enqueueNeedsSession", language).length).toBeGreaterThan(0);
      expect(queueText("enqueue", language).length).toBeGreaterThan(0);
    }
  });
});
