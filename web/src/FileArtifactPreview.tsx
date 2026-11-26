import { useEffect, useId, useRef } from "react";
import { useQuery } from "@tanstack/react-query";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import { X } from "lucide-react";
import { IconAction } from "./IconAction";
import type { FileArtifactRef } from "./fileArtifactLink";
import type { markdownMessages } from "./markdownMessages";

export function FileArtifactPreview({ artifact, sessionId, labels, onClose }: {
  artifact: FileArtifactRef;
  sessionId: string;
  labels: typeof markdownMessages.en;
  onClose: () => void;
}) {
  const dialog = useRef<HTMLDialogElement>(null);
  const title = useId();
  const query = useQuery({
    queryKey: ["session-file-preview", sessionId, artifact.path],
    queryFn: async ({ signal }) => {
      const path = new URLSearchParams({ path: artifact.path });
      const response = await fetch(`/api/sessions/${encodeURIComponent(sessionId)}/file-preview?${path}`, { signal });
      if (!response.ok) throw new Error(`Workspace file preview failed (${response.status})`);
      const result: unknown = await response.json();
      if (!result || typeof result !== "object" || !("content" in result) || typeof result.content !== "string") {
        throw new Error("Unexpected workspace file preview response");
      }
      return result.content;
    },
    retry: false,
    gcTime: 0,
  });

  useEffect(() => {
    const element = dialog.current;
    element?.showModal();
    return () => { if (element?.open) element.close(); };
  }, []);

  const close = () => {
    dialog.current?.close();
    onClose();
  };
  const isMarkdown = artifact.ext === "md" || artifact.ext === "markdown";
  return <dialog ref={dialog} className="attachment-dialog markdown-artifact-dialog" aria-labelledby={title}
    onCancel={(event) => { event.preventDefault(); close(); }}>
    <header>
      <div><small>{labels.preview}</small><h2 id={title}>{artifact.name}</h2></div>
      <IconAction label={labels.close} onClick={close}><X size={20} /></IconAction>
    </header>
    {query.isPending && <p role="status">{labels.loading}</p>}
    {query.isError && <p role="alert">{labels.unavailable}</p>}
    {query.isSuccess && (isMarkdown ? <div className="markdown-body markdown-artifact-document">
      <ReactMarkdown remarkPlugins={[remarkGfm]} skipHtml disallowedElements={["a", "img"]} unwrapDisallowed>
        {query.data}
      </ReactMarkdown>
    </div> : <pre tabIndex={0}>{query.data}</pre>)}
  </dialog>;
}
