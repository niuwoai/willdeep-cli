/**
 * Web 聊天区的简洁显示，对齐 TUI 的 `note_narration` / `note_reply`。
 *
 * 简洁是默认：只有会话显式选了 `detailed`（`/verbose`）才逐段显示。干活时中途
 * 文字不进聊天区，只留最近一段在一行会替换的状态上，下一段来了就换掉它；轮次
 * 收尾才落成正文。计划围栏两种模式都原位更新卡片，不进这行。中断或状态复位时，
 * 还挂在这行上的最后一段补写进聊天区，不随状态行一起消失。
 */

export type ReplyStyle = "short" | "detailed" | null | undefined;

export type ConciseLine = { text: string; startedAt: number };

/** 没显式选过风格即简洁，与 `ResponseStyle::uses_short` 同一条判定。 */
export function usesShortStyle(style: ReplyStyle): boolean {
  return style !== "detailed";
}

/**
 * 这段回复是不是只由计划围栏组成。围栏要原位更新卡片，普通中途文字才进状态行。
 * 只认完整的 ```plan / ```progress 围栏，半截或夹着别的围栏一律不算。
 */
export function isPlanReply(content: string): boolean {
  // 整段必须就是这一个围栏。前面还有一句话、半截或夹着别的围栏都不算：
  // 算了会把那句话一并丢掉，因为围栏本身不进聊天区。
  const fence = /^```(plan|progress)\n([\s\S]*?)\n```\s*$/.exec(content.trim());
  if (!fence) return false;
  const body = fence[2].split("\n").map((line) => line.trim()).filter(Boolean);
  if (!body.length) return false;
  if (fence[1] === "plan") {
    return body.every((line, index) => {
      const prefix = `${index + 1}. `;
      return line.startsWith(prefix) && line.slice(prefix.length).trim().length > 0;
    });
  }
  return body.every((line) => /^\S+: (pending|in_progress|done|skipped|failed|blocked)$/.test(line));
}

/** 一段定稿的中途文字。详细模式逐段落进聊天区；简洁模式只替换那一行。 */
export function narrationDecision(style: ReplyStyle, text: string): "plan" | "append" | "hold" {
  if (isPlanReply(text)) return "plan";
  return usesShortStyle(style) ? "hold" : "append";
}

/**
 * 收尾文字里还没显示过的部分。正文就是最后一段时只落一次；守护进程在前面拼了
 * 提示（`{提示}\n\n{最后一段}`）时，提示单独成段，最后一段照旧落。
 */
/** 守护进程把中途文字截到这么多字再发给页面（`web.rs` 的 `truncate`）。 */
const NARRATION_PREVIEW_CHARS = 240;

export function unshownReply(finalText: string, held: string | null): string | null {
  const text = finalText.trim();
  if (!text) return null;
  const previous = held?.trim();
  if (!previous) return text;
  if (text === previous) return null;
  // 截断过的状态行是正文的前缀：正文已经把它包含了，不再单独落一段。
  if (previous.length >= NARRATION_PREVIEW_CHARS && text.startsWith(previous)) return null;
  if (!text.endsWith(previous)) return text;
  const head = text.slice(0, -previous.length).trim();
  return head || null;
}

/** 轮次被中断或状态复位时，把还挂在状态行上的最后一段补成正文。 */
export function interruptedReply(held: ConciseLine | null): string | null {
  const text = held?.text.trim();
  return text ? text : null;
}

export type ConciseItem = { role: string; content: string; tools?: readonly unknown[] };

/**
 * 简洁模式回放时，一轮里最后一条助手正文之外的中途文字都不进聊天区。
 *
 * 干活时它们只该待在那一行状态上，轮次收尾才落成正文。计划卡片两种模式都保留。
 * 带工具调用的那条不整条隐掉，否则工具行会跟着没了。原始记录一个字不动。
 */
export function conciseHistory<T extends ConciseItem>(items: T[], style: ReplyStyle): T[] {
  if (!usesShortStyle(style)) return items;
  const hidden = new Set<number>();
  let roundStart = 0;
  const fold = (end: number) => {
    let lastAssistant = -1;
    for (let index = roundStart; index < end; index += 1) {
      if (items[index].role === "assistant" && items[index].content.trim()) lastAssistant = index;
    }
    for (let index = roundStart; index < end; index += 1) {
      const item = items[index];
      if (item.role === "assistant" && item.content.trim() && index !== lastAssistant && !item.tools?.length) hidden.add(index);
    }
  };
  items.forEach((item, index) => {
    if (item.role === "user") {
      fold(index);
      roundStart = index + 1;
    }
  });
  fold(items.length);
  return items.filter((_, index) => !hidden.has(index));
}
