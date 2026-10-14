type ActivityStep = { label: string; status: "active" | "done" | "failed" };

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

/** 运行卡片已展示的当前状态，无需在输入框上方再播报一次。 */
export function needsActivityStrip(activity: string, steps: ActivityStep[], thinking: string): boolean {
  return !steps.some((step, index) => {
    if (step.status !== "active" && index !== steps.length - 1) return false;
    return step.label === activity
      || (activity === thinking && step.label.startsWith(`${thinking} ·`));
  });
}
