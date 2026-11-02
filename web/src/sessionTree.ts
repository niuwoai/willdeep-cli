/** 一条会话在列表里排序所需的最小信息。 */
export type SessionNode = {
  id: string;
  /** 直接分凉来源。缺失、空串或指向自己都当没有来源。 */
  started_by?: string | null;
};

export type SessionTreeRow<T extends SessionNode> = {
  session: T;
  /** 0 是顶层，1 是挂在直接来源下的分凉。再往下分凉仍然只算一层。 */
  depth: number;
};

/**
 * 把分凉出来的会话挪到它直接来源的正下方，保持各自原来的相对顺序。
 *
 * 来源不在这份列表里（别的工作区、已归档、被搜索掉或已删除）就留在原位，
 * 当普通会话，从不隐藏。环在这里断开：一条会话只挪一次，挪到直接来源后面。
 */
export function sessionTree<T extends SessionNode>(sessions: T[]): SessionTreeRow<T>[] {
  const present = new Set(sessions.map((session) => session.id));
  const parentOf = (session: T): string | null => {
    const parent = session.started_by;
    if (!parent || parent === session.id || !present.has(parent)) return null;
    return parent;
  };

  // 逐层插入。先按原顺序放下来源不在列表里的顶层，再一轮轮把分凉插到它直接来源的
  // 后面。同一轮里按原顺序处理，所以同一来源的几条分凉保持原来的相对顺序；一轮只挂
  // 来源已经就位的，孙级要等它的直接来源先挂上，不会抢到同级前面。
  //
  // 一轮里谁都挂不上时，把剩下的按原来的位置留一条下来：来源不在列表里的会话这样
  // 留在原位，下一轮还能收住它自己的分凉。指向另一条还没放置的会话才算环，环的成员
  // 不接收分凉，于是环在这里断开，两条都留在顶层。
  const placed: T[] = [];
  const placedIds = new Set<string>();
  // 真正挂到来源下面的。兜底留在原位的不在这里，也不缩进。
  const nested = new Set<string>();
  // 环的成员。不往它们下面挂，否则环的另一头会被当成可挂接的来源。
  const parked = new Set<string>();
  for (const session of sessions) {
    if (parentOf(session) === null) {
      placed.push(session);
      placedIds.add(session.id);
      // 来源是空串的会话在 parentOf 里和没有来源一样，但它自己还在列表里。放进 parked，
      // 它的分凉就不会挂上来，跟指向一个不在列表里的来源一样留在原位。
      if (session.started_by === "") parked.add(session.id);
    }
  }
  const awaiting = (): T[] => sessions.filter((session) => !placedIds.has(session.id));
  let progressed = true;
  while (progressed) {
    progressed = false;
    const pending = awaiting();
    const attachable = pending.some((session) => {
      const parent = parentOf(session);
      return parent !== null && placedIds.has(parent) && !parked.has(parent);
    });
    if (!attachable) {
      const session = pending[0];
      if (session === undefined) break;
      const index = sessions.indexOf(session);
      const ahead = sessions.slice(0, index).filter((earlier) => placedIds.has(earlier.id)).length;
      // 插到第 ahead 个已放置会话之后。splice 插在下标处、原元素后移，下标不能超过当前长度。
      placed.splice(Math.min(Math.max(ahead, placed.length), placed.length), 0, session);
      placedIds.add(session.id);
      const parent = parentOf(session);
      const unresolved = parent !== null && sessions.some((candidate) => candidate.id === parent && !placedIds.has(candidate.id));
      // parent 为 null 有两种：真的没有来源，和来源是空串。前者不在列表里，不会有人挂上来；
      // 后者要放进 parked 挡住它的分凉。这里只挡还没放置的来源，也就是环。
      if (unresolved) parked.add(session.id);
      progressed = true;
      continue;
    }
    for (const session of pending) {
      const parent = parentOf(session);
      if (parent === null || !placedIds.has(parent) || parked.has(parent)) continue;
      const parentAt = placed.findIndex((candidate) => candidate.id === parent);
      let insertAt = parentAt + 1;
      while (insertAt < placed.length && parentOf(placed[insertAt]) === parent) insertAt += 1;
      placed.splice(insertAt, 0, session);
      placedIds.add(session.id);
      nested.add(session.id);
      progressed = true;
    }
  }

  // 缩进看最终的排列，不看声明。紧挨着来源，或紧挨着一条已经挂在同一来源下的分凉，
  // 才算一层；再往下分凉的只挂在直接来源下，所以也只算一层。中间隔着顶层会话的不
  // 缩进，否则竖线会跨过不属于这个来源的行。
  const depths = placed.map(() => 0);
  placed.forEach((session, index) => {
    if (!nested.has(session.id) || index === 0) return;
    const parent = parentOf(session);
    const previous = placed[index - 1];
    if (previous.id === parent || (depths[index - 1] === 1 && parentOf(previous) === parent)) {
      depths[index] = 1;
    }
  });
  return placed.map((session, index) => ({ session, depth: depths[index] }));
}
