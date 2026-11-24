import { useContext, useEffect, useRef, useState } from "react";
import { Check, Copy } from "lucide-react";
import type { MarkdownCopyLabels } from "./markdownCopy";
import { writeClipboard } from "./markdownClipboard";
import { CopyLabelsContext } from "./markdownCopyContext";
import type { CopyFieldMode } from "./copyField";

const COPIED_VISIBLE_MS = 2000;

export function CopyFieldCard({ mode, text }: { mode: CopyFieldMode; text: string }) {
  const copy = useContext(CopyLabelsContext);
  const [state, setState] = useState<"idle" | "copied" | "failed">("idle");
  const timer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  useEffect(() => () => clearTimeout(timer.current), []);

  async function onCopy() {
    try {
      await writeClipboard(text);
      setState("copied");
    } catch {
      setState("failed");
    }
    clearTimeout(timer.current);
    timer.current = setTimeout(() => setState("idle"), COPIED_VISIBLE_MS);
  }

  const feedback = copyFeedback(copy, state);
  return (
    <div className={`markdown-copy-field markdown-copy-field-${mode}${state === "idle" ? "" : " has-feedback"}`}>
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
      <div className={`markdown-copy-field-body${mode === "textarea" ? " is-multiline" : ""}`} tabIndex={0} role="group" aria-label={text}>
        {text}
      </div>
    </div>
  );
}

function copyFeedback(copy: MarkdownCopyLabels, state: "idle" | "copied" | "failed") {
  return state === "copied" ? copy.copied : state === "failed" ? copy.failed : "";
}
