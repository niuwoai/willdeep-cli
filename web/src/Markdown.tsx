import { memo, useContext, useEffect, useRef, useState, type ComponentPropsWithoutRef, type ReactNode } from "react";
import ReactMarkdown, { type Components } from "react-markdown";
import remarkGfm from "remark-gfm";
import { Check, Copy } from "lucide-react";
import { type MarkdownCopyLabels } from "./markdownCopy";
import { CopyLabelsContext } from "./markdownCopyContext";
import { copyFieldFromPre } from "./copyField";
import { CopyFieldCard } from "./CopyFieldCard";
import { MarkdownLink } from "./FileArtifactChip";
import { writeClipboard } from "./markdownClipboard";

const remarkPlugins = [remarkGfm];
const COPIED_VISIBLE_MS = 2000;

const components: Components = {
  a: MarkdownLink,
  pre({ node, children, ...props }) {
    void node;
    const field = copyFieldFromPre(children as ReactNode);
    if (field) return <CopyFieldCard mode={field.mode} text={field.text} />;
    return <CodeBlock {...props}>{children}</CodeBlock>;
  },
};

// 长会话里一次轮询会让整个聊天区重渲染；正文没变就别重新解析 Markdown。
// `copy` 由 markdownCopy 按语言缓存，引用稳定时这里才能真正跳过。
export const Markdown = memo(function Markdown({ content, copy }: { content: string; copy: MarkdownCopyLabels }) {
  return (
    <CopyLabelsContext.Provider value={copy}>
      <div className="markdown-body">
        <ReactMarkdown remarkPlugins={remarkPlugins} components={components}>
          {content}
        </ReactMarkdown>
      </div>
    </CopyLabelsContext.Provider>
  );
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
  return (
    <div className={`markdown-codeblock${state === "idle" ? "" : " has-feedback"}`}>
      <button
        type="button"
        className={`markdown-copy${state === "idle" ? "" : ` ${state}`}`}
        aria-label={copy.copy}
        title={feedback || copy.copy}
        onClick={() => void onCopy()}
      >
        {state === "copied" ? <Check size={14} aria-hidden="true" /> : <Copy size={14} aria-hidden="true" />}
        <span className="markdown-copy-feedback" role="status" aria-live="polite">
          {feedback}
        </span>
      </button>
      <pre ref={ref} {...props}>
        {children}
      </pre>
    </div>
  );
}
