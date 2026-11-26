import { useEffect, useState, type RefObject } from "react";

export const CHAT_SELECTION_ACTIONS = "[data-chat-selection-actions]";
const SELECTION_GAP = 6;
const QUOTE_PREFIX = "> ";

type ChatSelection = {
  scope: string;
  text: string;
  x: number;
  y: number;
  focusFirst?: boolean;
};

export function appendSelectionQuote(draft: string, selectedText: string) {
  const separator = !draft || draft.endsWith("\n\n") ? "" : draft.endsWith("\n") ? "\n" : "\n\n";
  const quote = selectedText.split("\n").map((line) => `${QUOTE_PREFIX}${line}`).join("\n");
  return `${draft}${separator}${quote}\n\n`;
}

export function composerCommandText(draft: string) {
  // A separate Markdown quote makes the draft a chat message, including after edits or restoration.
  const text = draft.trim();
  if (!text.startsWith("/") || /\n[ \t]*\n {0,3}>/.test(text)) return "";
  return text;
}

export function useChatSelection(viewportRef: RefObject<HTMLElement | null>, scope: string) {
  const [snapshot, setSelection] = useState<ChatSelection | null>(null);

  useEffect(() => {
    setSelection(null);
    const activeViewport = viewportRef.current;
    if (!activeViewport) return;
    const viewport = activeViewport;
    let selecting = false;
    let lastSelection: { start: Node; end: Node; startOffset: number; endOffset: number; text: string } | null = null;
    const actionsHaveFocus = () => Boolean(document.activeElement?.closest(CHAT_SELECTION_ACTIONS));
    const inViewport = (event: Event) => event.target instanceof Node && viewport.contains(event.target);
    const dismiss = () => setSelection(null);

    function readSelection(range: Range, text: string, options: { onlyIfChanged?: boolean; focusFirst?: boolean; point?: { x: number; y: number } } = {}) {
      if (!viewport.contains(range.commonAncestorContainer) || !text.trim()) {
        lastSelection = null;
        dismiss();
        return;
      }
      const { startContainer: start, endContainer: end, startOffset, endOffset } = range;
      const rect = range.getBoundingClientRect();
      const bounds = viewport.getBoundingClientRect();
      if (rect.bottom < bounds.top || rect.top > bounds.bottom) {
        dismiss();
        return;
      }
      if (options.onlyIfChanged && lastSelection?.start === start && lastSelection.end === end
          && lastSelection.startOffset === startOffset && lastSelection.endOffset === endOffset && lastSelection.text === text) return;
      lastSelection = { start, end, startOffset, endOffset, text };
      const point = options.point ?? { x: rect.left, y: Math.min(rect.bottom, bounds.bottom) + SELECTION_GAP };
      setSelection({ scope, text, ...point, focusFirst: options.focusFirst });
      return true;
    }

    function liveSelection() {
      const selection = window.getSelection();
      if (!selection || selection.isCollapsed || !selection.rangeCount) return null;
      return { range: selection.getRangeAt(0), text: selection.toString() };
    }

    function onSelectionChange() {
      if (selecting) return;
      const next = liveSelection();
      if (next) {
        readSelection(next.range, next.text, { onlyIfChanged: true });
        return;
      }
      // Focusing the actions or replacing Markdown during streaming can collapse the native range.
      if (actionsHaveFocus() || (lastSelection && (!lastSelection.start.isConnected || !lastSelection.end.isConnected))) return;
      lastSelection = null;
      dismiss();
    }

    function onPointerDown(event: PointerEvent) {
      if (event.button !== 0) return;
      if (event.target instanceof Element && event.target.closest(CHAT_SELECTION_ACTIONS)) return;
      selecting = inViewport(event);
      dismiss();
    }

    function onPointerUp() {
      if (!selecting) return;
      selecting = false;
      const next = liveSelection();
      if (next) readSelection(next.range, next.text);
    }

    function onPointerCancel() {
      selecting = false;
    }

    function onContextMenu(event: MouseEvent) {
      const target = event.target;
      if (event.defaultPrevented || !(target instanceof Node) || !viewport.contains(target)) return;
      const next = liveSelection();
      if (!next || !next.range.intersectsNode(target)) return;
      if (readSelection(next.range, next.text, { point: { x: event.clientX, y: event.clientY } })) event.preventDefault();
    }

    function onKeyDown(event: KeyboardEvent) {
      const contextKey = event.key === "ContextMenu" || (event.shiftKey && event.key === "F10");
      if (event.defaultPrevented || !contextKey || actionsHaveFocus()) return;
      const focus = document.activeElement;
      if (focus !== document.body && !inViewport(event)) return;
      const next = liveSelection();
      if (!next || !focus?.contains(next.range.commonAncestorContainer)) return;
      if (readSelection(next.range, next.text, { focusFirst: true })) event.preventDefault();
    }

    document.addEventListener("selectionchange", onSelectionChange);
    document.addEventListener("pointerdown", onPointerDown);
    document.addEventListener("pointerup", onPointerUp);
    document.addEventListener("pointercancel", onPointerCancel);
    document.addEventListener("contextmenu", onContextMenu);
    document.addEventListener("keydown", onKeyDown);
    viewport.addEventListener("scroll", dismiss, true);
    window.addEventListener("resize", dismiss);
    return () => {
      document.removeEventListener("selectionchange", onSelectionChange);
      document.removeEventListener("pointerdown", onPointerDown);
      document.removeEventListener("pointerup", onPointerUp);
      document.removeEventListener("pointercancel", onPointerCancel);
      document.removeEventListener("contextmenu", onContextMenu);
      document.removeEventListener("keydown", onKeyDown);
      viewport.removeEventListener("scroll", dismiss, true);
      window.removeEventListener("resize", dismiss);
    };
  }, [scope, viewportRef]);

  return [snapshot?.scope === scope ? snapshot : null, setSelection] as const;
}
