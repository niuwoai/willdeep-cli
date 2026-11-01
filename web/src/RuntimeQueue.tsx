import { useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { ArrowUp, Clock3, GitBranch, Image as ImageIcon, Paperclip, Pencil, Trash2 } from "lucide-react";
import type { Language } from "./i18n";
import { queueText } from "./queueMessages";
import { Markdown } from "./Markdown";
import { queueCopyLabels } from "./markdownCopy";
import type { ConversationItem } from "./ConversationCard";
import { queueRequest } from "./runtimeQueueApi";

type IndependentTask = { id: string; status: string; messages: ConversationItem[] };
export function IndependentTasks({ sessionId, language }: { sessionId: string | null; language: Language }) {
  const t = (name: Parameters<typeof queueText>[0]) => queueText(name, language);
  const tasks = useQuery({ queryKey: ["independent-tasks", sessionId], enabled: Boolean(sessionId), refetchInterval: 1000,
    queryFn: ({ signal }) => queueRequest<IndependentTask[]>(`/api/sessions/${encodeURIComponent(sessionId ?? "")}/independent`, { signal }) });
  if (tasks.error) return <p role="alert">{t("syncFailed")}</p>;
  if (!tasks.data?.length) return null;
  return <section aria-label={t("independent")} className="queued-messages"><header><strong>{t("independent")}</strong></header>
    <p>{t("originalRetained")}</p>
    {tasks.data.map((task) => <article key={task.id}>
      <small>{task.status === "completed" ? t("completedShort") : task.status === "running" || task.status === "queued" ? t("runningShort") : task.status.startsWith("waiting_") ? t("waitingShort") : t("failedTask")}</small>
      <Markdown copy={queueCopyLabels(t("copyCode"), t("copiedCode"), t("copyFailed"))} content={task.messages.filter((message) => message.role === "user" || message.role === "assistant").map((message) => `**${message.role === "user" ? t("you") : t("assistant")}**\n\n${message.content}`).join("\n\n---\n\n")} />
    </article>)}
  </section>;
}

export type RuntimeQueueItem = {
  id: string; session_id: string; text: string; text_preview: string;
  can_edit: boolean; image_count: number; text_attachment_count: number;
};
export function RuntimeQueue({ sessionId, language }: { sessionId: string | null; language: Language }) {
  const client = useQueryClient();
  const key = ["runtime-queue", sessionId];
  const url = `/api/sessions/${encodeURIComponent(sessionId ?? "")}/queue`;
  const queue = useQuery({ queryKey: key, enabled: Boolean(sessionId),
    queryFn: ({ signal }) => queueRequest<RuntimeQueueItem[]>(url, { signal }), refetchInterval: 1000 });
  const [editing, setEditing] = useState<RuntimeQueueItem | null>(null);
  const [draft, setDraft] = useState("");
  const t = (name: Parameters<typeof queueText>[0]) => queueText(name, language);
  const mutation = useMutation({
    mutationFn: (body: { action: "edit" | "remove" | "send_now" | "run_independently"; message_id: string; text?: string; expected_text?: string }) =>
      queueRequest<RuntimeQueueItem[]>(url, { method: "PATCH", headers: { "content-type": "application/json" }, body: JSON.stringify(body) }),
    onSuccess: () => { setEditing(null); void client.invalidateQueries({ queryKey: key }); },
    onError: () => { void client.invalidateQueries({ queryKey: key }); },
  });
  if (!sessionId || (!queue.data?.length && !editing && !queue.error && !mutation.error)) return null;
  return <section className="queued-messages runtime-queue" aria-label={t("title")}>
    {queue.error && <p role="alert">{t("syncFailed")}</p>}
    {mutation.error && <p role="alert">{t("mutationFailed")}</p>}
    {editing && queue.data && !queue.data.some((item) => item.id === editing.id) && <div className="queue-editor">
      <p>{t("editedUnavailable")}</p>
      <textarea aria-label={t("edit")} value={draft} onChange={(event) => setDraft(event.target.value)} />
      <button onClick={() => setEditing(null)}>{t("cancel")}</button>
    </div>}
    {!!queue.data?.length && <><header><strong>{t("title")}</strong><span className="management-count">{queue.data.length}</span></header>
      <ol>{queue.data.map((item) => { const editingItem = editing?.id === item.id && editing.session_id === sessionId;
        return <li key={item.id}><p className="queue-message-content">{item.text_preview}</p>
        {(item.image_count > 0 || item.text_attachment_count > 0) && <p className="queue-attachment-count">
          {item.image_count > 0 && <span><ImageIcon size={12} aria-hidden="true" />{t("images")} {item.image_count}</span>}
          {item.text_attachment_count > 0 && <span><Paperclip size={12} aria-hidden="true" />{t("attachments")} {item.text_attachment_count}</span>}
        </p>}
        {editingItem && <div className="queue-editor">
          <textarea aria-label={t("edit")} value={draft} onChange={(event) => setDraft(event.target.value)} />
          <div className="management-editor-actions">
            <button type="button" disabled={mutation.isPending} onClick={() => setEditing(null)}>{t("cancel")}</button>
            <button type="button" className="management-primary" disabled={mutation.isPending || (!draft.trim() && !item.image_count && !item.text_attachment_count)}
              onClick={() => mutation.mutate({ action: "edit", message_id: item.id, text: draft, expected_text: editing.text })}>{t("save")}</button>
          </div>
        </div>}
        <div className="queue-row-actions">
          <button type="button" className="queue-verb" disabled={mutation.isPending} onClick={() => mutation.mutate({ action: "send_now", message_id: item.id })}>
            <ArrowUp size={13} aria-hidden="true" />{t("prioritize")}<small>{t("prioritizeHint")}</small>
          </button>
          <button type="button" className="queue-verb" disabled={mutation.isPending} onClick={() => mutation.mutate({ action: "run_independently", message_id: item.id })}>
            <GitBranch size={13} aria-hidden="true" />{t("runIndependent")}<small>{t("runIndependentHint")}</small>
          </button>
          <span className="queue-row-spacer" />
          {!editingItem && <button type="button" className="queue-verb quiet" disabled={mutation.isPending || !item.can_edit} onClick={() => { setEditing(item); setDraft(item.text); }}>
            <Pencil size={13} aria-hidden="true" />{t("edit")}
          </button>}
          <button type="button" className="queue-verb quiet danger" disabled={mutation.isPending} onClick={() => mutation.mutate({ action: "remove", message_id: item.id })}>
            <Trash2 size={13} aria-hidden="true" />{t("delete")}
          </button>
        </div>
      </li>; })}</ol>
      <p className="queue-policy"><Clock3 size={12} aria-hidden="true" />{t("dependencyPolicy")}</p></>}
  </section>;
}
