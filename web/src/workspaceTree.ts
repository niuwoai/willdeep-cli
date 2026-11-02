/** 一个工作区在侧栏树里排序所需的最小信息。 */
export type WorkspaceNode = {
  id: string;
  path: string;
  /** 最后一次有会话更新的时间，秒。从没有过会话的工作区是 0。 */
  latest: number;
};

/** 每个项目默认露出的会话数。 */
export const PROJECT_PREVIEW = 5;
/** 「显示更多」每次加载的会话数。 */
export const PROJECT_PAGE = 10;
/** 一个项目最多列出的会话数。再旧的不进树，搜索仍能找到。 */
export const PROJECT_CAP = 20;

/**
 * 工作区按最近活动排序：最近有会话更新的在最上面。
 *
 * 从没有过会话的工作区排到最后，互相保持传入时的相对顺序。
 */
export function orderWorkspaces<T extends WorkspaceNode>(workspaces: T[]): T[] {
  return workspaces
    .map((workspace, index) => ({ workspace, index }))
    .sort((a, b) => b.workspace.latest - a.workspace.latest || a.index - b.index)
    .map((entry) => entry.workspace);
}

/**
 * 一个项目当前该列出多少条会话。
 *
 * `shown` 是用户要求的条数：默认预览是 {@link PROJECT_PREVIEW}，每次「显示更多」
 * 加 {@link PROJECT_PAGE}，「收起」回到预览。总数先裁到 {@link PROJECT_CAP}，再旧的不进树。
 */
export function visibleCount(total: number, shown: number): number {
  const capped = Math.min(Math.max(0, total), PROJECT_CAP);
  const requested = Math.max(PROJECT_PREVIEW, shown);
  return Math.min(capped, requested);
}

/** 「显示更多」之后该记下的条数：一次加一页，不超过上限。 */
export function afterShowMore(shown: number): number {
  return Math.min(PROJECT_CAP, Math.max(PROJECT_PREVIEW, shown) + PROJECT_PAGE);
}

/**
 * 侧栏搜索按会话标题匹配，大小写不敏感。
 *
 * 标题还是占位符时，列表上显示的是首条用户消息（`preview`），
 * 那段字就是用户看到的标题，一并参与匹配。空查询匹配全部。
 */
export function matchesSessionQuery(session: { title: string; preview?: string }, query: string): boolean {
  const needle = query.trim().toLowerCase();
  if (!needle) return true;
  if (session.title.toLowerCase().includes(needle)) return true;
  return (session.preview ?? "").toLowerCase().includes(needle);
}

/**
 * 一个项目这一次该列出多少条。
 *
 * 搜索时列出全部命中，不受 {@link PROJECT_CAP} 限制：上限只约束不搜索时的树。
 */
export function listedSessionCount(total: number, shown: number, searching: boolean): number {
  if (searching) return Math.max(0, total);
  return visibleCount(total, shown);
}
