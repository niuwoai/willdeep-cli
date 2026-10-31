import { describe, expect, it } from "vitest";
import { afterShowMore, listedSessionCount, matchesSessionQuery, orderWorkspaces, PROJECT_CAP, PROJECT_PAGE, PROJECT_PREVIEW, visibleCount, type WorkspaceNode } from "./workspaceTree";

function workspace(id: string, latest: number): WorkspaceNode {
  return { id, path: `/${id}`, latest };
}

describe("工作区树", () => {
  it("按最近会话更新排序，最新的在最上面", () => {
    const ordered = orderWorkspaces([
      workspace("old", 10),
      workspace("fresh", 30),
      workspace("quiet", 0),
      workspace("mid", 20),
    ]);
    expect(ordered.map((item) => item.id)).toEqual(["fresh", "mid", "old", "quiet"]);
  });

  it("活动时间相同时保持传入顺序", () => {
    expect(orderWorkspaces([workspace("a", 5), workspace("b", 5)]).map((item) => item.id)).toEqual(["a", "b"]);
  });

  it("从没有过会话的工作区排在最后", () => {
    expect(orderWorkspaces([workspace("empty", 0), workspace("used", 1)]).map((item) => item.id)).toEqual(["used", "empty"]);
  });
});

describe("项目下的会话数量", () => {
  it("默认只露预览数量", () => {
    expect(visibleCount(40, PROJECT_PREVIEW)).toBe(PROJECT_PREVIEW);
  });

  it("不足预览数量时全部列出", () => {
    expect(visibleCount(2, PROJECT_PREVIEW)).toBe(2);
    expect(visibleCount(0, PROJECT_PREVIEW)).toBe(0);
  });

  it("显示更多一次加一页", () => {
    const once = afterShowMore(PROJECT_PREVIEW);
    expect(once - PROJECT_PREVIEW).toBe(PROJECT_PAGE);
    expect(visibleCount(40, once)).toBe(PROJECT_PREVIEW + PROJECT_PAGE);
  });

  it("加到上限就停，再旧的不进树", () => {
    const twice = afterShowMore(afterShowMore(PROJECT_PREVIEW));
    expect(visibleCount(40, twice)).toBe(PROJECT_CAP);
    expect(afterShowMore(PROJECT_CAP)).toBe(PROJECT_CAP);
  });

  it("总数不足一页时只列到总数", () => {
    expect(visibleCount(8, afterShowMore(PROJECT_PREVIEW))).toBe(8);
  });

  it("搜索时不裁到上限，不搜索时仍走预览", () => {
    expect(listedSessionCount(40, PROJECT_PREVIEW, true)).toBe(40);
    expect(listedSessionCount(40, PROJECT_PREVIEW, false)).toBe(PROJECT_PREVIEW);
  });
});

describe("按标题搜索", () => {
  it("匹配标题，忽略大小写和两端空白", () => {
    expect(matchesSessionQuery({ title: "Fix Login" }, "  login ")).toBe(true);
  });

  it("标题还是占位符时，匹配侧栏上显示的首条消息", () => {
    expect(matchesSessionQuery({ title: "New session", preview: "帮我看登录失败" }, "登录")).toBe(true);
  });

  it("对不上标题也对不上显示文本时不命中", () => {
    expect(matchesSessionQuery({ title: "发版记录", preview: "打个 tag" }, "登录")).toBe(false);
  });

  it("空查询不过滤", () => {
    expect(matchesSessionQuery({ title: "任意" }, "   ")).toBe(true);
  });
});
