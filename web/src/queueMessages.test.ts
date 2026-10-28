import { describe, expect, it } from "vitest";
import { enqueueButtonState, queueText } from "./queueMessages";

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
