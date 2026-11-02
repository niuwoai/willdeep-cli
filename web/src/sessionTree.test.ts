import { describe, expect, it } from "vitest";
import { sessionTree, type SessionNode } from "./sessionTree";

function row(id: string, startedBy?: string): SessionNode {
  return { id, started_by: startedBy ?? null };
}

function layout(sessions: SessionNode[]): string[] {
  return sessionTree(sessions).map((entry) => `${entry.depth}:${entry.session.id}`);
}

describe("会话谱系", () => {
  it("分凉会话紧跟来源，不管它自己更新得更晚", () => {
    expect(layout([row("fork", "parent"), row("other"), row("parent")])).toEqual([
      "0:other",
      "0:parent",
      "1:fork",
    ]);
  });

  it("同一来源的几条分凉保持原来的相对顺序", () => {
    expect(layout([row("b", "parent"), row("parent"), row("a", "parent")])).toEqual([
      "0:parent",
      "1:b",
      "1:a",
    ]);
  });

  it("来源不在这份列表里时留在原位，不隐藏", () => {
    expect(layout([row("fork", "missing"), row("plain")])).toEqual(["0:fork", "0:plain"]);
    expect(layout([row("self", "self")])).toEqual(["0:self"]);
  });

  it("分凉的分凉挂在直接来源下，仍然只缩进一层", () => {
    expect(layout([row("grandchild", "child"), row("child", "parent"), row("parent")])).toEqual([
      "0:parent",
      "1:child",
      "1:grandchild",
    ]);
  });

  it("互相指向的两条断开环，两条都留在顶层", () => {
    expect(layout([row("a", "b"), row("b", "a")])).toEqual(["0:a", "0:b"]);
  });

  it("没有来源的列表原样返回", () => {
    expect(layout([row("one"), row("two")])).toEqual(["0:one", "0:two"]);
  });
});
