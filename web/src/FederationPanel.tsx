// 联邦面板：配对、联系人、发消息。
//
// 独立于公网入口的站点/表单配置：联系人表在 `federation.json`，不进 config.toml。
// 只在回环 Web 上可用，和旁边几页同一套边界。

import { useEffect, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Text } from "@chakra-ui/react";
import type { Language } from "./i18n";
import {
  invitePeer,
  joinPeer,
  loadFederation,
  removePeer,
  sendToPeer,
  type FederationContact,
  type FederationView,
} from "./federationApi";
import { federationText } from "./federationMessages";
import "./intake.css";

export function FederationPanel({ language, onOpenSession }: { language: Language; onOpenSession: (sessionId: string) => void }) {
  const t = federationText(language);
  const client = useQueryClient();
  const query = useQuery({ queryKey: ["federation"], queryFn: ({ signal }) => loadFederation(signal) });
  const [view, setView] = useState<FederationView | null>(null);
  const [notice, setNotice] = useState<{ tone: "ok" | "error"; text: string } | null>(null);
  const [invite, setInvite] = useState<string>("");
  const [joinUrl, setJoinUrl] = useState("");
  const [drafts, setDrafts] = useState<Record<string, string>>({});
  const [busy, setBusy] = useState(false);

  useEffect(() => { if (query.data) setView(query.data); }, [query.data]);

  async function run(action: () => Promise<void>) {
    setBusy(true);
    setNotice(null);
    try { await action(); } catch (reason) { setNotice({ tone: "error", text: reason instanceof Error ? reason.message : String(reason) }); }
    finally { setBusy(false); }
  }

  if (query.isLoading) return <section className="intake-section"><Text>{t.loading}</Text></section>;
  if (query.error) return <section className="intake-section"><Text role="alert">{t.loadFailed}: {query.error.message}</Text></section>;
  if (!view) return <section className="intake-section"><Text>{t.loading}</Text></section>;

  const apply = (next: FederationView) => { setView(next); client.setQueryData(["federation"], next); };

  return <section className="intake-section">
    <p className="intake-hint">{t.hint}</p>
    {!view.enabled && <p className="intake-notice">{t.disabled}</p>}
    <div className="intake-card federation-me">
      <div className="intake-card-title"><strong>{t.me}</strong><Text as="code">{view.me.name} · {view.me.id.slice(0, 8)}</Text></div>
      {!view.invite_base && <p className="intake-hint intake-wide">{t.noPublicUrl}</p>}
    </div>

    <div className="intake-card">
      <div className="intake-card-title"><strong>{t.pairing}</strong></div>
      <div className="intake-wide intake-row">
        <button type="button" className="intake-add" disabled={busy || !view.invite_base} onClick={() => void run(async () => {
          const created = await invitePeer();
          setInvite(created.url);
          setNotice({ tone: "ok", text: t.inviteReady });
        })}>{t.invite}</button>
        <input value={joinUrl} placeholder={t.joinPlaceholder} onChange={(event) => setJoinUrl(event.target.value)} />
        <button type="button" disabled={busy || !joinUrl.trim() || !view.invite_base} onClick={() => void run(async () => {
          apply(await joinPeer(joinUrl.trim()));
          setJoinUrl("");
          setNotice({ tone: "ok", text: t.joined });
        })}>{t.join}</button>
      </div>
      {invite && <div className="intake-wide federation-invite">
        <code>{invite}</code>
        <span className="intake-hint">{t.inviteHint}</span>
      </div>}
    </div>

    {notice && <p role={notice.tone === "error" ? "alert" : "status"} className={notice.tone === "error" ? "intake-error" : "intake-notice"}>{notice.text}</p>}

    {view.contacts.length === 0 && <p>{t.empty}</p>}
    {view.contacts.map((contact) => <ContactCard key={contact.id} contact={contact} draft={drafts[contact.id] ?? ""} busy={busy}
      onDraft={(text) => setDrafts((current) => ({ ...current, [contact.id]: text }))}
      onSend={() => void run(async () => {
        await sendToPeer(contact.id, drafts[contact.id] ?? "");
        setDrafts((current) => ({ ...current, [contact.id]: "" }));
        setNotice({ tone: "ok", text: t.sent });
      })}
      onRemove={() => void run(async () => { apply(await removePeer(contact.id)); })}
      onOpenSession={onOpenSession} t={t} />)}
  </section>;
}

function ContactCard({ contact, draft, busy, onDraft, onSend, onRemove, onOpenSession, t }: {
  contact: FederationContact; draft: string; busy: boolean;
  onDraft: (text: string) => void; onSend: () => void; onRemove: () => void;
  onOpenSession: (sessionId: string) => void; t: ReturnType<typeof federationText>;
}) {
  return <div className="intake-card">
    <div className="intake-card-title">
      <strong>{contact.name}<span className="federation-short"> · {contact.short_id}</span></strong>
      <Text as="code">{contact.url}</Text>
    </div>
    <div className="intake-wide intake-row">
      <textarea rows={2} value={draft} placeholder={t.messagePlaceholder} disabled={busy} onChange={(event) => onDraft(event.target.value)} />
      <button type="button" disabled={busy || !draft.trim()} onClick={onSend}>{t.send}</button>
    </div>
    <div className="intake-card-actions">
      {contact.session_id && <button type="button" className="intake-link" onClick={() => onOpenSession(contact.session_id as string)}>{t.openSession}</button>}
      <span className="intake-hint">{t.autoTurns.replace("{count}", String(contact.auto_turns))}</span>
      <button type="button" className="intake-danger" disabled={busy} onClick={onRemove}>{t.remove}</button>
    </div>
  </div>;
}
