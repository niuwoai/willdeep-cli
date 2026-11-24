import type { ComponentPropsWithoutRef } from "react";
import { fileArtifactFromHref, fileArtifactGlyph, type FileArtifactRef } from "./fileArtifactLink";
import { markdownNodeText } from "./copyField";
import type { ReactNode } from "react";

export function MarkdownLink({ node, href, children, ...props }: ComponentPropsWithoutRef<"a"> & { children?: ReactNode; node?: unknown }) {
  void node;
  const label = markdownNodeText(children).trim();
  const artifact = href ? fileArtifactFromHref(href, label) : null;
  if (artifact) return <FileArtifactChip artifact={artifact} href={href!} />;
  return (
    <a {...props} href={href} target="_blank" rel="noopener noreferrer">
      {children}
    </a>
  );
}

function FileArtifactChip({ artifact, href }: { artifact: FileArtifactRef; href: string }) {
  return (
    <a className="markdown-file-chip" href={href} title={artifact.path}>
      <span className="markdown-file-chip-icon" aria-hidden="true">
        {fileArtifactGlyph(artifact.ext)}
      </span>
      <span className="markdown-file-chip-name">{artifact.name}</span>
    </a>
  );
}
