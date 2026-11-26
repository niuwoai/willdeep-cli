import { useContext, useRef, useState, type ComponentPropsWithoutRef } from "react";
import { ExternalLink } from "lucide-react";
import { createPortal } from "react-dom";
import { fileArtifactFromHref, fileArtifactGlyph, isBrowserLink } from "./fileArtifactLink";
import { MarkdownMessageContext } from "./markdownCopyContext";
import { FileArtifactPreview } from "./FileArtifactPreview";

export function MarkdownLink({ node, href, children, title, ...props }: ComponentPropsWithoutRef<"a"> & { node?: unknown }) {
  void node;
  const { sessionId, labels } = useContext(MarkdownMessageContext);
  const trigger = useRef<HTMLButtonElement>(null);
  const [preview, setPreview] = useState<string | null>(null);
  const artifact = href ? fileArtifactFromHref(href) : null;
  if (artifact) {
    if (!sessionId) return <span title={artifact.path}>{children}</span>;
    const previewKey = JSON.stringify([sessionId, artifact.path]);
    return <>
      <button ref={trigger} type="button" className="markdown-file-chip" title={artifact.path}
        aria-label={`${labels.preview} · ${artifact.name}`} aria-haspopup="dialog" onClick={() => setPreview(previewKey)}>
        <span className="markdown-file-chip-icon" aria-hidden="true">{fileArtifactGlyph(artifact.ext)}</span>
        <span className="markdown-file-chip-name">{artifact.name}</span>
      </button>
      {preview === previewKey && createPortal(
        <FileArtifactPreview artifact={artifact} sessionId={sessionId} labels={labels} onClose={() => {
          setPreview(null);
          trigger.current?.focus();
        }} />,
        document.body,
      )}
    </>;
  }
  if (!href) return <span>{children}</span>;
  const browserLink = isBrowserLink(href);
  return (
    <a {...props} href={href} title={title ?? (browserLink ? labels.openLink : undefined)}
      target={href.startsWith("#") ? undefined : "_blank"} rel="noopener noreferrer">
      {children}
      {browserLink && <ExternalLink className="markdown-link-open" size={14} aria-hidden="true" />}
    </a>
  );
}
