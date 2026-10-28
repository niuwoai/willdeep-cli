import { createContext, memo, useContext, useEffect, useRef, useState, type ComponentPropsWithoutRef } from "react";
import ReactMarkdown, { type Components } from "react-markdown";
import remarkGfm from "remark-gfm";
import { Check, Copy } from "lucide-react";
import { type MarkdownCopyLabels } from "./markdownCopy";

const remarkPlugins = [remarkGfm];
const COPIED_VISIBLE_MS = 2000;

const CopyLabelsContext = createContext<MarkdownCopyLabels>({
  copy: "复制", copied: "已复制", failed: "复制失败",
});

const components: Components = {
  a({ node, ...props }) {
    void node;
    return <a {...props} target="_blank" rel="noopener noreferrer" />;
  },
  pre({ node, children, ...props }) {
    void node;
    return <CodeBlock {...props}>{children}</CodeBlock>;
  },
};

// 长会话里一次轮询会让整个聊天区重渲染；正文没变就别重新解析 Markdown。
// `copy` 由 markdownCopy 按语言缓存，引用稳定时这里才能真正跳过。
export const Markdown = memo(function Markdown({ content, copy }: { content: string; copy: MarkdownCopyLabels }) {
  return <CopyLabelsContext.Provider value={copy}>
    <div className="markdown-body">
      <ReactMarkdown remarkPlugins={remarkPlugins} components={components}>{content}</ReactMarkdown>
    </div>
  </CopyLabelsContext.Provider>;
});

function CodeBlock({ children, ...props }: ComponentPropsWithoutRef<"pre">) {
  const copy = useContext(CopyLabelsContext);
  const ref = useRef<HTMLPreElement>(null);
  const [state, setState] = useState<"idle" | "copied" | "failed">("idle");
  const timer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  useEffect(() => () => clearTimeout(timer.current), []);

  async function onCopy() {
    const text = (ref.current?.textContent ?? "").replace(/\n$/, "");
    try {
      await writeClipboard(text);
      setState("copied");
    } catch {
      setState("failed");
    }
    clearTimeout(timer.current);
    timer.current = setTimeout(() => setState("idle"), COPIED_VISIBLE_MS);
  }

  const feedback = state === "copied" ? copy.copied : state === "failed" ? copy.failed : "";
  return <div className={`markdown-codeblock${state === "idle" ? "" : " has-feedback"}`}>
    <button type="button" className={`markdown-copy${state === "idle" ? "" : ` ${state}`}`} aria-label={copy.copy} title={feedback || copy.copy} onClick={() => void onCopy()}>
      {state === "copied" ? <Check size={14} aria-hidden="true" /> : <Copy size={14} aria-hidden="true" />}
      <span className="markdown-copy-feedback" role="status" aria-live="polite">{feedback}</span>
    </button>
    <pre ref={ref} {...props}>{children}</pre>
  </div>;
}

async function writeClipboard(text: string) {
  try {
    if (navigator.clipboard?.writeText) {
      await navigator.clipboard.writeText(text);
      return;
    }
  } catch {
    // 非安全上下文或被拒绝时改走选区复制。
  }
  const area = document.createElement("textarea");
  area.value = text;
  area.setAttribute("readonly", "");
  area.style.position = "fixed";
  area.style.left = "-9999px";
  document.body.appendChild(area);
  area.select();
  let ok = false;
  try { ok = document.execCommand("copy"); } catch { ok = false; }
  area.remove();
  if (!ok) throw new Error("copy failed");
}
