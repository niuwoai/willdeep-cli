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

  it("委派父级优先于历史分叉来源", () => {
    expect(layout([{ id: "child", parent_session_id: "parent", started_by: "fork-source" }, row("fork-source"), row("parent")])).toEqual(["0:fork-source", "0:parent", "1:child"]);
    expect(layout([{ id: "orphan", parent_session_id: "missing", started_by: "fork-source" }, row("fork-source")])).toEqual(["0:orphan", "0:fork-source"]);
  });

  it("只搜索到子会话或父级成环时仍能访问每条会话", () => {
    expect(layout([{ id: "child", parent_session_id: "parent" }])).toEqual(["0:child"]);
    const cycle = [{ id: "a", parent_session_id: "b" }, { id: "b", parent_session_id: "a" }];
    expect(layout(cycle)).toEqual(["0:a", "0:b"]);
  });

  it("已脱离会话忽略委派父级和历史分叉，按独立条目原序展示", () => {
    expect(layout([{ id: "detached", parent_session_id: "parent", started_by: "fork-source", delegation_detached: true }, row("other"), row("fork-source"), row("parent")])).toEqual(["0:detached", "0:other", "0:fork-source", "0:parent"]);
    expect(layout([row("parent"), row("other"), { id: "detached", parent_session_id: "parent", started_by: "parent", delegation_detached: true }])).toEqual(["0:parent", "0:other", "0:detached"]);
  });

  it("脱离只解除自身的树形父级，仍能展示自己的后续分叉", () => {
    expect(layout([row("fork", "detached"), row("parent"), { id: "detached", parent_session_id: "parent", delegation_detached: true }])).toEqual(["0:parent", "0:detached", "1:fork"]);
  });

  it("没有来源的列表原样返回", () => {
    expect(layout([row("one"), row("two")])).toEqual(["0:one", "0:two"]);
  });
});
