import { memo, useContext, useEffect, useRef, useState, type ComponentPropsWithoutRef, type ReactNode } from "react";
import ReactMarkdown, { defaultUrlTransform, type Components, type UrlTransform } from "react-markdown";
import remarkGfm from "remark-gfm";
import { Check, Copy } from "lucide-react";
import { type MarkdownCopyLabels } from "./markdownCopy";
import { CopyLabelsContext, MarkdownMessageContext } from "./markdownCopyContext";
import { markdownMessages } from "./markdownMessages";
import type { Language } from "./i18n";
import { fileArtifactFromHref, isBrowserLink } from "./fileArtifactLink";
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

const markdownUrl: UrlTransform = (href, key, node) => {
  if (key !== "href" || node.tagName !== "a") return defaultUrlTransform(href);
  if (fileArtifactFromHref(href)) return href;
  if (/^https?:/i.test(href)) return isBrowserLink(href) ? href : "";
  // Local paths are rendered only as preview buttons, never browser navigations.
  return /^(?:#|[a-z][a-z\d+.-]*:)/i.test(href) ? defaultUrlTransform(href) : "";
};

// `copy` is cached by language so long transcripts can skip polling updates.
export const Markdown = memo(function Markdown({ content, copy, language, sessionId }: {
  content: string;
  copy: MarkdownCopyLabels;
  language?: Language;
  sessionId?: string | null;
}) {
  return (
    <CopyLabelsContext.Provider value={copy}>
      <MarkdownMessageContext.Provider value={{ sessionId, labels: markdownMessages[language ?? copy.language ?? "en"] }}>
        <div className="markdown-body">
          <ReactMarkdown remarkPlugins={remarkPlugins} components={components} urlTransform={markdownUrl}>
            {content}
          </ReactMarkdown>
        </div>
      </MarkdownMessageContext.Provider>
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
