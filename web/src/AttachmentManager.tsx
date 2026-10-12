import { useEffect, useRef, useState } from "react";
import type { Language } from "./i18n";
import { attachmentText } from "./attachmentMessages";
import { attachmentError, MAX_ATTACHMENTS, moveAttachment, type DraftAttachment } from "./attachmentModel";
import { ArrowDown, ArrowUp, CopyPlus, FileText, MoreHorizontal, Pencil, Trash2, X } from "lucide-react";
import { IconAction } from "./IconAction";

export function AttachmentManager({ items, onChange, language, readOnly = false, onCopy, readOnlyNotice }: {
  items: DraftAttachment[]; onChange: (items: DraftAttachment[]) => void; language: Language;
  readOnly?: boolean; onCopy?: (item: DraftAttachment) => void; readOnlyNotice?: string;
}) {
  const t = (key: Parameters<typeof attachmentText>[0]) => attachmentText(key, language);
  const [selected, setSelected] = useState<string | null>(null);
  const [editing, setEditing] = useState(false);
  const [text, setText] = useState("");
  const [error, setError] = useState("");
  const dialog = useRef<HTMLDialogElement>(null);
  const opener = useRef<HTMLElement | null>(null);
  const item = items.find((entry) => entry.id === selected);
  const open = (entry: DraftAttachment, edit: boolean) => { opener.current = document.activeElement instanceof HTMLElement ? document.activeElement : null; setSelected(entry.id); setEditing(edit); setText(entry.kind === "text" ? entry.content : ""); setError(""); };
  const close = () => { setSelected(null); setEditing(false); dialog.current?.close(); opener.current?.focus(); };
  useEffect(() => {
    if (item && !dialog.current?.open) dialog.current?.showModal(); else if (!item && dialog.current?.open) dialog.current.close();
    if (item && editing) dialog.current?.querySelector("textarea")?.focus();
  }, [item, editing]);
  const save = () => {
    if (!item || item.kind !== "text") return;
    const updated = { ...item, content: text };
    const validation = attachmentError(updated);
    if (validation) { setError(t(validation)); return; }
    onChange(items.map((entry) => entry.id === item.id ? updated : entry)); close();
  };
  if (!items.length) return null;
  return <section className={`attachment-manager${readOnly ? " readonly" : ""}`} aria-label={t("manage")}><header><span className="attachment-count">{t("title")}<span className="management-count">{items.length}</span><small>{t("capacity").replace("{limit}", String(MAX_ATTACHMENTS))}</small></span>
    {readOnly ? <small className="attachment-readonly">{readOnlyNotice ?? t("readonly")}</small> : <IconAction label={t("clear")} className="danger-action" onClick={() => { if (window.confirm(t("clearConfirm"))) onChange([]); }}><Trash2 size={15} /></IconAction>}</header>
    <ol>{items.map((entry, index) => <li key={entry.id}><button type="button" className="attachment-preview-button" onClick={() => open(entry, false)} title={t("preview")}>
      {entry.kind === "image" ? <img src={`data:${entry.media_type};base64,${entry.data}`} alt={entry.name} /> : <span className="attachment-file-icon"><FileText size={19} /></span>}<span className="attachment-description"><span className="attachment-name">{entry.name}</span><small>{entry.kind === "image" ? `${t("image")} · ${entry.width} × ${entry.height}` : `${t("text")} · ${t("characters").replace("{count}", String(Array.from(entry.content).length))}`}</small></span>
    </button><div className="attachment-controls">
      {!readOnly && <>{entry.kind === "text" && <IconAction label={t("edit")} onClick={() => open(entry, true)}><Pencil size={16} /></IconAction>}
        <details className="management-more"><summary aria-label={`${t("more")}: ${entry.name}`} title={t("more")}><MoreHorizontal size={17} /></summary><div className="management-menu">
          <button type="button" aria-label={`${t("up")}: ${entry.name}`} disabled={index === 0} onClick={(event) => { onChange(moveAttachment(items, entry.id, -1)); event.currentTarget.closest("details")?.removeAttribute("open"); }}><ArrowUp size={15} />{t("up")}</button>
          <button type="button" aria-label={`${t("down")}: ${entry.name}`} disabled={index === items.length - 1} onClick={(event) => { onChange(moveAttachment(items, entry.id, 1)); event.currentTarget.closest("details")?.removeAttribute("open"); }}><ArrowDown size={15} />{t("down")}</button>
        </div></details>
        <IconAction label={`${t("delete")}: ${entry.name}`} className="danger-action" onClick={() => onChange(items.filter((candidate) => candidate.id !== entry.id))}><X size={16} /></IconAction></>}
      {readOnly && onCopy && <IconAction label={t("copy")} onClick={() => onCopy(entry)}><CopyPlus size={17} /></IconAction>}
    </div></li>)}</ol>
    <dialog ref={dialog} className="attachment-dialog" onCancel={(event) => { event.preventDefault(); close(); }} aria-label={t("preview")}><header><div><small>{editing ? t("edit") : t("preview")}</small><h2>{item?.name}</h2></div><IconAction label={t("close")} onClick={close}><X size={20} /></IconAction></header>
      {item?.kind === "image" && <img className="attachment-full-image" src={`data:${item.media_type};base64,${item.data}`} alt={item.name} />}
      {item?.kind === "text" && (editing ? <><textarea aria-label={t("edit")} value={text} onChange={(event) => setText(event.target.value)} />{error && <p role="alert">{error}</p>}<footer className="management-editor-actions"><button type="button" onClick={close}>{t("cancel")}</button><button type="button" className="management-primary" onClick={save}>{t("save")}</button></footer></> : <pre>{item.content}</pre>)}
    </dialog>
  </section>;
}
