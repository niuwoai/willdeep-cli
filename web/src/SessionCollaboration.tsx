import { useRef, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { ArrowUpRight, Check, ChevronDown, MessageSquare, RefreshCw, Send, X } from "lucide-react";
import type { Language } from "./i18n";
import { collaborationText } from "./collaborationMessages";
import { useInspectorDismiss } from "./useInspectorDismiss";
import "./sessionCollaboration.css";

type Peer = { id: string; title: string | null; status: string; workspace: string; branch?: string | null; current_step?: string | null; last_user_request: string | null };
type Note = { id: string; from_session: string; to_session: string; body: string; reply_to: string | null; consumed_at: number | null; read_by: string[] };
const drafts = new Map<string, string>();

async function request<T>(path: string, init?: RequestInit): Promise<T> {
  const response = await fetch(path, init);
  if (!response.ok) {
    const body: unknown = await response.json().catch(() => null);
    const detail = body && typeof body === "object" && "error" in body && typeof body.error === "string" ? body.error : "";
    throw new Error(detail ? `${response.status}: ${detail}` : String(response.status));
  }
  return await response.json() as T;
}

export function SessionCollaboration({ sessionId, language, onOpenSession }: {
  sessionId: string; language: Language; onOpenSession: (id: string) => void;
}) {
  const t = collaborationText(language);
  const panel = useInspectorDismiss();
  const cache = useQueryClient();
  const base = `/api/sessions/${encodeURIComponent(sessionId)}`;
  const peers = useQuery({ queryKey: ["session-peers", sessionId], queryFn: ({ signal }) => request<Peer[]>(`${base}/peers`, { signal }), refetchInterval: 5000 });
  const inbox = useQuery({ queryKey: ["peer-messages", sessionId], queryFn: ({ signal }) => request<Note[]>(`${base}/peer-messages`, { signal }), refetchInterval: 5000 });
  const [target, setTarget] = useState("");
  const [draft, setDraft] = useState("");
  const [replyTo, setReplyTo] = useState<string | null>(null);
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const requestIdentity = useRef<{ fingerprint: string; id: string } | null>(null);
  const updateDraft = (text: string) => { drafts.set(`${sessionId}:${target}`, text); setDraft(text); };
  const chooseTarget = (id: string, reply: string | null = null) => {
    setTarget(id); setReplyTo(reply); setDraft(drafts.get(`${sessionId}:${id}`) ?? "");
  };
  const refresh = async () => { await Promise.all([cache.invalidateQueries({ queryKey: ["session-peers", sessionId] }), cache.invalidateQueries({ queryKey: ["peer-messages", sessionId] })]); };
  const send = async () => {
    if (!target || !draft.trim() || pending || Array.from(draft.trim()).length > 4000) return;
    const captured = draft;
    const key = `${sessionId}:${target}`;
    const fingerprint = JSON.stringify([sessionId, target, draft.trim(), replyTo]);
    if (requestIdentity.current?.fingerprint !== fingerprint) requestIdentity.current = { fingerprint, id: crypto.randomUUID() };
    setPending(true); setError(null);
    try {
      await request(`${base}/peer-messages`, { method: "POST", headers: { "content-type": "application/json" },
        body: JSON.stringify({ session_id: target, message: draft.trim(), reply_to: replyTo, request_id: requestIdentity.current.id }) });
      if (drafts.get(key) === captured) { drafts.delete(key); setDraft((current) => current === captured ? "" : current); }
      setReplyTo(null); requestIdentity.current = null;
      await refresh();
    } catch (reason) { setError(reason instanceof Error ? reason.message : String(reason)); }
    finally { setPending(false); }
  };
  const markRead = async (id: string) => {
    try { await request(`${base}/peer-messages/${encodeURIComponent(id)}/read`, { method: "POST" }); await refresh(); }
    catch (reason) { setError(reason instanceof Error ? reason.message : String(reason)); }
  };
  const notes = inbox.data ?? [];
  const unread = notes.filter((note) => note.to_session === sessionId && !note.read_by.includes("web")).length;
  const title = (id: string) => peers.data?.find((peer) => peer.id === id)?.title || id.slice(0, 8);
  return <aside className="session-collaboration"><details ref={panel} className="conversation-disclosure">
    <summary aria-label={unread > 0 ? `${t.title} · ${t.unread} ${unread}` : t.title} title={t.title}><MessageSquare size={16} /><span>{t.title}</span>{unread > 0 && <span className="peer-unread" aria-label={`${t.unread} ${unread}`}>{unread}</span>}<ChevronDown size={13} /></summary>
    <div className="conversation-inspector peer-inspector">
    <div className="inspector-heading"><div><h2>{t.title}</h2><p>{t.subtitle}</p></div><div className="inspector-controls"><button className="inspector-icon" type="button" aria-label={t.refresh} title={t.refresh} onClick={() => void refresh()}><RefreshCw size={16} /></button><button className="inspector-icon" type="button" aria-label={t.close} title={t.close} onClick={() => { if (panel.current) { panel.current.open = false; panel.current.querySelector("summary")?.focus(); } }}><X size={16} /></button></div></div>
    {(peers.isPending || inbox.isPending) && <p role="status">{t.loading}</p>}
    {(peers.error || inbox.error || error) && <p role="alert">{t.failed}: {error ?? peers.error?.message ?? inbox.error?.message}</p>}
    <section className="peer-compose">
    <label>{t.target}<select value={target} disabled={pending} onChange={(event) => chooseTarget(event.target.value)}>
      <option value="">{t.choose}</option>
      {(peers.data ?? []).filter((peer) => peer.id !== sessionId).map((peer) => <option value={peer.id} key={peer.id}>{peer.title || peer.last_user_request || peer.id.slice(0, 8)}</option>)}
    </select></label>
    {(peers.data ?? []).filter((peer) => peer.id === target).map((peer) => <details className="peer-context" key={peer.id}><summary>{peer.title || t.target}<span>{Object.entries(t.sessionStates).find(([key]) => key === peer.status)?.[1] ?? t.sessionStates.unknown}</span></summary><dl>
      <dt>{t.status}</dt><dd>{Object.entries(t.sessionStates).find(([key]) => key === peer.status)?.[1] ?? t.sessionStates.unknown}</dd>
      <dt>{t.workspace}</dt><dd>{peer.workspace}</dd>
      {peer.branch && <><dt>{t.branch}</dt><dd>{peer.branch}</dd></>}
      {peer.last_user_request && <><dt>{t.request}</dt><dd>{peer.last_user_request}</dd></>}
      {peer.current_step && <><dt>{t.step}</dt><dd>{peer.current_step}</dd></>}
    </dl></details>)}
    <label>{t.draft}<textarea placeholder={t.placeholder} value={draft} onChange={(event) => updateDraft(event.target.value)} aria-describedby="peer-limit" /></label>
    <div className="peer-compose-footer"><span id="peer-limit" title={t.limit}>{Array.from(draft).length}/4000</span>
    {replyTo && <button type="button" onClick={() => setReplyTo(null)}>{t.cancelReply}</button>}
    <button className="peer-send" type="button" disabled={pending || !target || !draft.trim() || Array.from(draft.trim()).length > 4000} onClick={() => void send()}><Send size={14} />{pending ? t.sending : t.send}</button></div>
    </section>
    <div className="peer-history-heading">{t.history}<span>{notes.length}</span></div>
    {!notes.length && !inbox.isPending && <div className="inspector-empty"><MessageSquare size={24} /><p>{t.empty}</p></div>}
    <ol className="peer-history">{notes.map((note) => {
      const incoming = note.to_session === sessionId;
      const peer = incoming ? note.from_session : note.to_session;
      const replied = notes.some((other) => other.reply_to === note.id);
      return <li key={note.id}>
        <header><strong>{incoming ? t.from : t.to}: {title(peer)}</strong><span className={`peer-delivery${note.consumed_at != null || replied ? " delivered" : ""}`}>{replied ? t.replied : note.consumed_at != null ? t.consumed : t.queued}</span></header>
        <p className="peer-body">{note.body}</p>
        <div className="peer-actions">
          <button type="button" onClick={() => onOpenSession(peer)}>{t.open}<ArrowUpRight size={13} /></button>
          {incoming && <button type="button" disabled={pending} onClick={() => chooseTarget(peer, note.id)}>{t.reply}</button>}
          {incoming && !note.read_by.includes("web") && <button type="button" onClick={() => void markRead(note.id)}><Check size={13} />{t.markRead}</button>}
        </div>
      </li>;
    })}</ol><p className="peer-trust-hint">{t.hint}</p>
    </div>
  </details></aside>;
}
