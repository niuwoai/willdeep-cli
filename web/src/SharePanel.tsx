// 会话分享面板。与授权模式同一个 disclosure 外壳：工具栏一个按钮，点开是浮层。
//
// 这里只做本机的事：创建/撤销链接、把完整 URL 拼出来。分享页本身由公网入口
// 那个监听提供，只有对话，没有配置和插件。

import { useEffect, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { Share2, X } from "lucide-react";
import type { Language } from "./i18n";
import { shareText } from "./shareMessages";
import { createShare, loadShare, removeShare, shareUrl, ShareFailure, type ShareMode, type ShareView } from "./shareApi";
import { intakeBase, intakeBaseIsLoopback, loadIntake } from "./intakeApi";
import { useInspectorDismiss } from "./useInspectorDismiss";
import "./permissionsPanel.css";

export function SharePanel({ sessionId, language, open, onOpenChange }: {
  sessionId: string;
  language: Language;
  open: boolean;
  onOpenChange: (open: boolean) => void;
}) {
  const t = shareText(language);
  const panel = useInspectorDismiss();
  const share = useQuery({ queryKey: ["share", sessionId], queryFn: ({ signal }) => loadShare(sessionId, signal), enabled: open });
  const intake = useQuery({ queryKey: ["intake"], queryFn: ({ signal }) => loadIntake(signal), enabled: open });
  const [mode, setMode] = useState<ShareMode>("read");
  const [acknowledged, setAcknowledged] = useState(false);
  const [pending, setPending] = useState(false);
  const [copied, setCopied] = useState(false);
  const [notice, setNotice] = useState<{ tone: "ok" | "error"; text: string } | null>(null);

  useEffect(() => {
    const node = panel.current;
    if (node && node.open !== open) node.open = open;
  }, [open, panel]);

  useEffect(() => { if (!open) { setNotice(null); setCopied(false); setAcknowledged(false); } }, [open]);

  const current: ShareView | null = share.data?.share ?? null;
  const url = current ? shareUrl(current, intake.data ?? null) : null;
  // 配置了公网地址也要看它解析到哪儿：填成回环地址时链接照样只有本机能开。
  const loopbackBase = intake.data ? intakeBaseIsLoopback(intakeBase(intake.data)) : false;

  async function create() {
    if (pending) return;
    setPending(true); setNotice(null);
    try {
      await createShare(sessionId, mode);
      share.refetch().catch(() => undefined);
      setNotice({ tone: "ok", text: t.created });
    } catch (reason) {
      setNotice({ tone: "error", text: `${t.createFailed}: ${detail(reason)}` });
    } finally { setPending(false); }
  }

  async function stop() {
    if (pending) return;
    setPending(true); setNotice(null);
    try {
      await removeShare(sessionId);
      share.refetch().catch(() => undefined);
      setNotice({ tone: "ok", text: t.removed });
    } catch (reason) {
      setNotice({ tone: "error", text: `${t.removeFailed}: ${detail(reason)}` });
    } finally { setPending(false); }
  }

  async function copy() {
    if (!url) return;
    try { await navigator.clipboard.writeText(url); setCopied(true); }
    catch (reason) { setNotice({ tone: "error", text: `${t.copyFailed}: ${detail(reason)}` }); }
  }

  return <aside className="conversation-permissions">
    <details ref={panel} className="conversation-disclosure" onToggle={(event) => onOpenChange(event.currentTarget.open)}>
      <summary aria-label={t.title} title={t.title}>
        <Share2 size={16} />
      </summary>
      <div className="conversation-inspector permissions-inspector share-inspector">
        <div className="inspector-heading">
          <div><h2>{t.title}</h2><p>{t.subtitle}</p></div>
          <div className="inspector-controls">
            <button className="inspector-icon" type="button" aria-label={t.title} title={t.title} onClick={() => onOpenChange(false)}><X size={16} /></button>
          </div>
        </div>

        {notice && <p role={notice.tone === "error" ? "alert" : "status"} className={notice.tone === "error" ? "permissions-failure" : "permissions-notice"}>{notice.text}</p>}
        {share.isLoading && <p className="permissions-blocked">{t.loading}</p>}
        {share.error && <p role="alert" className="permissions-failure">{t.loadFailed}: {share.error.message}</p>}

        {intake.data && !intake.data.running && <p className="permissions-blocked" role="note">{t.needsIntake}</p>}
        {intake.data?.running && loopbackBase && <p className="permissions-blocked" role="note">{t.loopbackOnly}</p>}

        {current ? <>
          <dl className="permissions-current">
            <dt>{t.link}</dt>
            <dd>{url ? <a href={url} target="_blank" rel="noreferrer" className="share-link">{url}</a> : <span className="permissions-blocked">{t.needsIntake}</span>}</dd>
            <dt>{t.mode}</dt>
            <dd>{current.mode === "chat" ? t.chat : t.read}</dd>
            <dt>{t.created_at}</dt>
            <dd>{new Date(current.created_at * 1000).toLocaleString(language)}</dd>
          </dl>
          <div className="permissions-footer">
            <button type="button" className="permissions-apply" disabled={!url} onClick={() => void copy()}>{copied ? t.copied : t.copy}</button>
            <button type="button" className="share-stop" disabled={pending} onClick={() => void stop()}>{t.stop}</button>
          </div>
          {current.mode === "chat" && <p className="permissions-warning" role="note">{t.chatWarning}</p>}
        </> : <>
          <fieldset className="permissions-options" disabled={pending}>
            <legend>{t.mode}</legend>
            {(["read", "chat"] as const).map((option) => (
              <label key={option} className={`permissions-option${mode === option ? " selected" : ""}`}>
                <input type="radio" name="share-mode" value={option} checked={mode === option} onChange={() => { setMode(option); setAcknowledged(false); }} />
                <span><strong>{option === "chat" ? t.chat : t.read}</strong><small>{option === "chat" ? t.chatHint : t.readHint}</small></span>
              </label>
            ))}
          </fieldset>
          {mode === "chat" && <label className="permissions-ack">
            <input type="checkbox" checked={acknowledged} onChange={(event) => setAcknowledged(event.target.checked)} />
            {t.chatWarning}
          </label>}
          <div className="permissions-footer">
            <button type="button" className="permissions-apply" disabled={pending || (mode === "chat" && !acknowledged)} onClick={() => void create()}>{pending ? t.creating : t.create}</button>
          </div>
        </>}
      </div>
    </details>
  </aside>;
}

function detail(reason: unknown): string {
  return reason instanceof ShareFailure ? reason.message : reason instanceof Error ? reason.message : String(reason);
}
