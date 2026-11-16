import { useCallback, useEffect, useState, type FormEvent } from "react";
import { createRoot } from "react-dom/client";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import type { Language } from "./i18n";
import { shareText } from "./shareMessages";
import "./share.css";

type ShareMessage = { role: "user" | "assistant"; content: string };
type ShareSnapshot = {
  title: string;
  writable: boolean;
  running: boolean;
  messages: ShareMessage[];
};

function browserLanguage(): Language {
  const language = navigator.language.toLowerCase();
  if (language.startsWith("zh")) return "zh-CN";
  if (language.startsWith("ja")) return "ja";
  return "en";
}

const token = decodeURIComponent(location.pathname.split("/").filter(Boolean).at(-1) ?? "");
const t = shareText(browserLanguage());

function SharePage() {
  const [snapshot, setSnapshot] = useState<ShareSnapshot | null>(null);
  const [input, setInput] = useState("");
  const [error, setError] = useState("");
  const [notice, setNotice] = useState("");
  const [sending, setSending] = useState(false);

  const load = useCallback(async () => {
    try {
      const response = await fetch(`/api/share/${encodeURIComponent(token)}`, { cache: "no-store" });
      if (!response.ok) {
        throw new Error(response.status === 404 ? t.pageExpired : t.pageReadFailed.replace("{status}", String(response.status)));
      }
      const data = await response.json() as ShareSnapshot;
      setSnapshot(data);
      setError("");
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : t.pageLoadFailed);
    }
  }, []);

  useEffect(() => {
    document.title = t.pageTitle;
  }, [t.pageTitle]);

  useEffect(() => {
    void load();
    const timer = window.setInterval(() => void load(), 2500);
    return () => window.clearInterval(timer);
  }, [load]);

  async function sendMessage(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    const message = input.trim();
    if (!message || sending) return;
    setSending(true);
    setError("");
    try {
      const response = await fetch(`/api/share/${encodeURIComponent(token)}/send`, {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ message }),
      });
      const body = await response.json().catch(() => ({})) as { error?: string };
      if (!response.ok) throw new Error(body.error || t.pageSendFailed);
      setInput("");
      setNotice(t.pageQueued);
      await load();
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : t.pageSendFailed);
    } finally {
      setSending(false);
    }
  }

  return (
    <div className="share-page">
      <header className="share-header">
        <div className="share-header-inner">
          <h1>{snapshot?.title || t.pageConversation}</h1>
          <div className="share-status" role="status" aria-live="polite">
            {snapshot ? `${snapshot.running ? `${t.pageReplying} · ` : ""}${snapshot.writable ? t.pageCanContinue : t.pageReadOnly}` : t.pageReading}
          </div>
        </div>
      </header>

      <main className="share-messages">
        {!snapshot && !error && <p className="share-empty">{t.pageReading}</p>}
        {snapshot?.messages.length === 0 && <p className="share-empty">{t.pageEmpty}</p>}
        {snapshot?.messages.map((message, index) => (
          <article className={`share-message ${message.role}`} key={`${message.role}-${index}`}>
            <div className="share-who">{message.role === "user" ? t.pageGuest : t.pageAssistant}</div>
            <div className="share-markdown">
              <ReactMarkdown remarkPlugins={[remarkGfm]} components={{
                a: ({ href, children }) => <a href={href} target="_blank" rel="noopener noreferrer">{children}</a>,
              }}>{message.content}</ReactMarkdown>
            </div>
          </article>
        ))}
      </main>

      <footer className="share-footer">
        <div className="share-footer-inner">
          <div className="share-error" role="alert">{error}</div>
          <div className="share-notice" role="status" aria-live="polite">{notice}</div>
          {snapshot?.writable && (
            <form className="share-composer" onSubmit={sendMessage}>
              <textarea
                aria-label={t.pageSend}
                placeholder={t.pageSendPlaceholder}
                maxLength={8000}
                rows={2}
                value={input}
                onChange={(event) => setInput(event.target.value)}
              />
              <button className="share-send" type="submit" disabled={sending || !input.trim()}>
                {sending ? t.pageSending : t.pageSend}
              </button>
            </form>
          )}
        </div>
      </footer>
    </div>
  );
}

createRoot(document.getElementById("root")!).render(<SharePage />);
