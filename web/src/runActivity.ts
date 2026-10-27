type ActivityStep = { label: string; status: "active" | "pending" | "done" | "failed" };

export type RunStep = ActivityStep & { id: string; detail?: string; startedAt: number; elapsedMs?: number };

/** 只合并显示数据，原始消息的 ID 和步骤仍用于接收运行时更新。正文和思考是分组边界。 */
export function groupActivityMessages<T extends { role: string; content: string; steps?: RunStep[]; thinking?: string }>(messages: T[]): T[] {
  const grouped: T[] = [];
  for (const message of messages) {
    const previous = grouped[grouped.length - 1];
    if (message.role === "activity" && previous?.role === "activity" && !previous.content && !previous.thinking) {
      grouped[grouped.length - 1] = { ...previous, content: message.content, thinking: message.thinking,
        steps: [...(previous.steps ?? []), ...(message.steps ?? [])] };
    } else grouped.push(message);
  }
  return grouped;
}

export function formatStepElapsed(elapsedMs: number): string {
  if (elapsedMs < 1000) return `${Math.max(1, Math.round(elapsedMs))}ms`;
  if (elapsedMs < 120_000) return `${(elapsedMs / 1000).toFixed(1)}s`;
  return `${(elapsedMs / 60_000).toFixed(1)}m`;
}

/**
 * 分隔线文案。「轮到你」只在它还是聊天区最后一条时成立：新输出一来就不再显示。
 * 记录里可能带着旧版本写进去的这几个字，先去掉再决定，避免显示两次。
 */
export function turnDividerText(content: string, yourTurn: string, isLast: boolean): string {
  const label = ` · ${yourTurn}`;
  const trimmed = content.endsWith(" ──") && content.slice(0, -3).endsWith(label)
    ? `${content.slice(0, -3 - label.length)} ──`
    : content;
  return isLast && trimmed.endsWith(" ──") ? `${trimmed.slice(0, -3)}${label} ──` : trimmed;
}

/** 运行卡片已展示的当前状态，无需在输入框上方再播报一次。 */
export function needsActivityStrip(activity: string, steps: ActivityStep[], thinking: string): boolean {
  return !steps.some((step, index) => {
    if (step.status !== "active" && index !== steps.length - 1) return false;
    return step.label === activity
      || (activity === thinking && step.label.startsWith(`${thinking} ·`));
  });
}
