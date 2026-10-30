import { describe, expect, it } from "vitest";
import { conciseHistory, interruptedReply, isPlanReply, narrationDecision, unshownReply, usesShortStyle, type ConciseItem } from "./conciseTurn";

describe("简洁显示的判定", () => {
  it("没选过风格和显式选简洁都算简洁，只有详细逐段显示", () => {
    expect(usesShortStyle(null)).toBe(true);
    expect(usesShortStyle(undefined)).toBe(true);
    expect(usesShortStyle("short")).toBe(true);
    expect(usesShortStyle("detailed")).toBe(false);
  });

  it("完整的计划围栏进卡片，半截、夹着别的围栏或普通文字不认", () => {
    expect(isPlanReply("```plan\n1. 看日志\n2. 改一行\n```")).toBe(true);
    expect(isPlanReply("```progress\n1: done\n2: in_progress\n```")).toBe(true);
    expect(isPlanReply("先说一句\n```plan\n1. 看日志\n```")).toBe(false);
    expect(isPlanReply("```plan\n1. 看日志")).toBe(false);
    expect(isPlanReply("```bash\necho hi\n```")).toBe(false);
    expect(isPlanReply("普通的一段话")).toBe(false);
  });

  it("简洁模式只留最近一段，详细模式逐段追加，计划围栏两种模式都进卡片", () => {
    expect(narrationDecision("short", "先看日志")).toBe("hold");
    expect(narrationDecision(null, "先看日志")).toBe("hold");
    expect(narrationDecision("detailed", "先看日志")).toBe("append");
    expect(narrationDecision("short", "```plan\n1. 看日志\n```")).toBe("plan");
    expect(narrationDecision("detailed", "```progress\n1: done\n```")).toBe("plan");
  });

  it("收尾与状态行相同只落一次，前面拼了提示就两段都留", () => {
    expect(unshownReply("写完了", "写完了")).toBeNull();
    expect(unshownReply("⚠ 轮次上限\n\n写完了", "写完了")).toBe("⚠ 轮次上限");
    expect(unshownReply("另一段", "写完了")).toBe("另一段");
    expect(unshownReply("写完了", null)).toBe("写完了");
    expect(unshownReply("   ", null)).toBeNull();
    // 守护进程把状态行截到 240 字；截断过的前缀不再单独落一段。
    const long = "很长的一段".repeat(80);
    expect(unshownReply(long, long.slice(0, 240))).toBeNull();
    expect(unshownReply("短正文", "短正")).toBe("短正文");
  });

  it("中断时把还挂着的最后一段补出来，空的不补", () => {
    expect(interruptedReply({ text: "写到一半", startedAt: 0 })).toBe("写到一半");
    expect(interruptedReply({ text: "   ", startedAt: 0 })).toBeNull();
    expect(interruptedReply(null)).toBeNull();
  });
});

describe("简洁模式的历史回放", () => {
  function item(role: string, content: string, tools = 0): ConciseItem & { content: string } {
    return { role, content, tools: tools ? Array.from({ length: tools }, () => ({ name: "run_command" })) : undefined };
  }

  it("一轮里只留最后一段正文，带工具调用的那条和计划卡片都留", () => {
    const items = [
      item("user", "开始"),
      item("assistant", "先看日志"),
      item("assistant", "", 2),
      item("assistant", "再改一行"),
      item("plan", ""),
      item("assistant", "最终结论"),
      item("user", "继续"),
      item("assistant", "这一轮的结论"),
    ];
    const shown = conciseHistory(items, null);
    // 空正文那条只带工具调用：折叠不能把它整条隐掉，否则工具行跟着没了。
    expect(shown.map((entry) => entry.content || entry.role)).toEqual(["开始", "assistant", "plan", "最终结论", "继续", "这一轮的结论"]);
    expect(shown[1].tools).toHaveLength(2);
    expect(items).toHaveLength(8);
  });

  it("详细模式一字不收", () => {
    const items = [item("user", "开始"), item("assistant", "先看日志"), item("assistant", "结论")];
    expect(conciseHistory(items, "detailed")).toEqual(items);
  });
});
