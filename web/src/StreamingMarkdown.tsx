import { useEffect, useRef, useState, type ComponentProps } from "react";
import { Markdown } from "./Markdown";

const MARKDOWN_REFRESH_MS = 100;

export function StreamingMarkdown({ content, live, ...props }: ComponentProps<typeof Markdown> & { live: boolean }) {
  const [visible, setVisible] = useState(content);
  const latest = useRef(content);
  const timer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  useEffect(() => {
    latest.current = content;
    if (!live || content === visible || timer.current !== undefined) return;
    timer.current = setTimeout(() => {
      timer.current = undefined;
      setVisible(latest.current);
    }, MARKDOWN_REFRESH_MS);
  }, [content, live, visible]);
  useEffect(() => () => clearTimeout(timer.current), []);

  return <Markdown {...props} content={live && content.startsWith(visible) ? visible : content} />;
}
