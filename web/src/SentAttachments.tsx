import { useState } from "react";
import { useQuery } from "@tanstack/react-query";
import type { Language } from "./i18n";
import { AttachmentManager } from "./AttachmentManager";
import { attachmentText } from "./attachmentMessages";
import type { AttachmentPayload, DraftAttachment } from "./attachmentModel";
import { scheduleJson } from "./scheduleTypes";
import { ChevronDown, ChevronUp, Paperclip } from "lucide-react";

export function SentAttachments({ sessionId, reference, language, onCopy }: {
  sessionId: string; reference: string; language: Language; onCopy: (item: DraftAttachment) => void;
}) {
  const [expanded, setExpanded] = useState(false);
  const data = useQuery({ queryKey: ["sent-attachments", sessionId, reference], enabled: expanded,
    queryFn: ({ signal }) => scheduleJson<{ attachments: AttachmentPayload[] }>(`/api/sessions/${encodeURIComponent(sessionId)}/attachments/${encodeURIComponent(reference)}`, { signal }), staleTime: Infinity });
  return <div className="sent-attachments"><button type="button" className="sent-attachment-toggle" aria-expanded={expanded} onClick={() => setExpanded((value) => !value)}><Paperclip size={14} />{attachmentText("preview", language)}{expanded ? <ChevronUp size={13} /> : <ChevronDown size={13} />}</button>
    {expanded && data.error && <p role="alert">{data.error.message}</p>}
    {expanded && data.data && <AttachmentManager items={data.data.attachments.map((item, index) => ({ ...item, id: `${reference}:${index}` }))} onChange={() => undefined} language={language} readOnly onCopy={onCopy} />}
  </div>;
}
