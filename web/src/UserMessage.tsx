import { useId, useLayoutEffect, useRef, useState } from "react";
import { ChevronDown, ChevronUp } from "lucide-react";
import type { Messages } from "./i18n";
import "./userMessage.css";

export function UserMessage({ content, messages: t, onToggle }: { content: string; messages: Messages; onToggle?: () => void }) {
  const id = useId();
  const body = useRef<HTMLDivElement>(null);
  const [overflow, setOverflow] = useState(false);
  const [expandedContent, setExpandedContent] = useState<string | null>(null);
  const expanded = expandedContent === content;
  useLayoutEffect(() => {
    const element = body.current;
    if (!element) return;
    const measure = () => {
      const style = getComputedStyle(element);
      const lines = Number(style.getPropertyValue("--user-preview-lines"));
      setOverflow(element.scrollHeight > parseFloat(style.lineHeight) * lines + 1);
    };
    measure();
    const observer = new ResizeObserver(measure);
    observer.observe(element);
    return () => observer.disconnect();
  }, [content]);
  return <div className="user-message-content">
    <div className="user-message-viewport" data-collapsed={overflow && !expanded}>
      <div id={id} ref={body} className="user-message-body">{content}</div>
    </div>
    {overflow && <button type="button" className="user-message-toggle" aria-expanded={expanded} aria-controls={id}
      onClick={() => {
        const viewport = body.current?.parentElement;
        const top = viewport?.getBoundingClientRect().top ?? 0;
        const scrollTop = viewport?.closest(".chat-viewport")?.getBoundingClientRect().top ?? 0;
        onToggle?.();
        setExpandedContent(expanded ? null : content);
        if (expanded && top < scrollTop) viewport?.scrollIntoView({ block: "start" });
      }}>
      {expanded ? t.userMessageLess : t.userMessageMore}
      {expanded ? <ChevronUp size={15} aria-hidden="true" /> : <ChevronDown size={15} aria-hidden="true" />}
    </button>}
  </div>;
}
