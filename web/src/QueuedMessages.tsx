import { useEffect, useRef, useState } from "react";
import type { Language } from "./i18n";
import { AttachmentManager } from "./AttachmentManager";
import type { DraftAttachment } from "./attachmentModel";
export type QueuedMessage = { id: string; prompt: string; attachments: DraftAttachment[]; workspace: string; sessionId: string; failed: boolean; sending?: boolean; requestBody?: string };
import { queueText } from "./queueMessages";
import { Check, ChevronDown, ChevronUp, Clock3, ListOrdered, Pencil, RotateCcw, Trash2, X } from "lucide-react";
import { IconAction } from "./IconAction";
export function QueuedMessages({ items, open, onOpen, onChange, language }: {
  items: QueuedMessage[]; open: boolean; onOpen: (value: boolean) => void; onChange: (items: QueuedMessage[]) => void; language: Language;
}) {
  const t = (key: Parameters<typeof queueText>[0]) => queueText(key, language);
  const [editing, setEditing] = useState<string | null>(null);
  const [draft, setDraft] = useState("");
  const editor = useRef<HTMLTextAreaElement>(null);
  useEffect(() => { if (editing) editor.current?.focus(); }, [editing]);
  if (!items.length) return null;
  return <section className={`queued-messages${open ? " expanded" : ""}`} aria-label={t("title")}><header><span className="queue-heading"><ListOrdered size={17} /><strong>{t("title")}</strong><span className="management-count">{items.length}</span></span><IconAction label={open ? t("close") : t("manage")} disabled={open && editing !== null} aria-expanded={open} onClick={() => onOpen(!open)}>{open ? <ChevronUp size={18} /> : <ChevronDown size={18} />}</IconAction></header>
    {!open && <div className="queue-summary"><span>{items[0].prompt}</span><small><Clock3 size={12} />{items[0].sending ? t("sending") : items[0].failed ? t("failedShort") : t("pendingShort")}</small></div>}
    {open && <><p className="queue-pause-note"><Clock3 size={13} />{t("paused")}</p><ol>{items.map((item, index) => <li key={item.id} className={item.failed ? "queue-item unconfirmed" : "queue-item"}><div className="queue-item-heading"><span className="queue-position">{index + 1}</span><small>{item.sending ? t("sending") : item.failed ? t("failedShort") : t("pendingShort")}</small><div className="queue-item-actions">
      <IconAction label={t("edit")} disabled={editing !== null || item.sending || item.failed || Boolean(item.requestBody)} onClick={() => { setEditing(item.id); setDraft(item.prompt); }}><Pencil size={16} /></IconAction>
      <IconAction label={t("delete")} className="danger-action" disabled={item.sending || editing === item.id} onClick={() => onChange(items.filter((entry) => entry.id !== item.id))}><Trash2 size={16} /></IconAction>
    </div></div>
      {editing === item.id ? <div className="queue-editor"><textarea ref={editor} aria-label={t("edit")} value={draft} onChange={(event) => setDraft(event.target.value)} /><div className="management-editor-actions"><button type="button" onClick={() => setEditing(null)}><X size={14} />{t("cancel")}</button><button type="button" className="management-primary" disabled={!draft.trim() && !item.attachments.length} onClick={() => { onChange(items.map((entry) => entry.id === item.id ? { ...entry, prompt: draft } : entry)); setEditing(null); }}><Check size={14} />{t("save")}</button></div></div> : <p className="queue-message-content">{item.prompt}</p>}
      {item.failed && <div className="queue-delivery-note"><p role="alert">{t("failed")}</p><button type="button" className="queue-retry" onClick={() => onChange(items.map((entry) => entry.id === item.id ? { ...entry, failed: false } : entry))}><RotateCcw size={14} />{t("retry")}</button></div>}
      <AttachmentManager readOnlyNotice={item.sending ? t("sending") : t("failed")} readOnly={item.sending || item.failed || Boolean(item.requestBody)} items={item.attachments} onChange={(attachments) => onChange(items.map((entry) => entry.id === item.id ? { ...entry, attachments } : entry))} language={language} />
    </li>)}</ol></>}
  </section>;
}
