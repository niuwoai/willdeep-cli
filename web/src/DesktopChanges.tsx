import { useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { FileDiff, RefreshCw, X } from "lucide-react";
import type { Language } from "./i18n";
import { desktopText } from "./desktopMessages";
import "./desktopWorkspace.css";

type ChangedFile = { path: string; old_path: string | null; additions: number; deletions: number; binary: boolean };
type Changes = {
  git: boolean;
  branch: string | null;
  snapshot: { id: string; files: ChangedFile[] } | null;
  session_paths: string[] | null;
};
type RuntimeVersion = { desktop_version: string; runtime_version: string; protocol_version: string };
const changesEndpoint = "/api/desktop/changes";

async function readDesktop<T>(path: string, signal: AbortSignal): Promise<T> {
  const response = await fetch(path, { signal });
  if (!response.ok) throw new Error(String(response.status));
  return await response.json() as T;
}

function changesParams(workspace: string, session: string) {
  const params = new URLSearchParams({ workspace });
  if (session) params.set("session", session);
  return params;
}

export function useDesktopChanges(workspace: string, session: string, revision: number, enabled: boolean) {
  return useQuery({
    queryKey: ["desktop-changes", workspace, session, revision],
    queryFn: ({ signal }) => readDesktop<Changes>(`${changesEndpoint}?${changesParams(workspace, session)}`, signal),
    enabled: enabled && Boolean(workspace),
    staleTime: 5000,
    refetchOnWindowFocus: true,
  });
}

export function DesktopVersions({ language, version }: { language: Language; version: string }) {
  const t = desktopText(language);
  const runtime = useQuery({
    queryKey: ["desktop-runtime-version"],
    queryFn: ({ signal }) => readDesktop<RuntimeVersion>("/api/desktop/runtime", signal),
    refetchInterval: 30_000,
  });
  return <div className="desktop-versions" title={runtime.data && t.protocolVersion.replace("{version}", runtime.data.protocol_version)}>
    {(version || runtime.data) && <span>{t.desktopVersion.replace("{version}", runtime.data?.desktop_version ?? version)}</span>}
    {runtime.isError ? <span>{t.runtimeUnavailable}</span> : runtime.data && <span>{t.runtimeVersion.replace("{version}", runtime.data.runtime_version)}</span>}
  </div>;
}

export function DesktopChangesToggle({ language, count, open, onToggle }: {
  language: Language; count: number; open: boolean; onToggle: () => void;
}) {
  const t = desktopText(language);
  const label = count > 0 ? t.changedFiles.replace("{count}", String(count)) : t.changes;
  return <button type="button" className="desktop-toolbar-button" title={label} aria-label={label}
    aria-expanded={open} aria-controls="desktop-changes" onClick={onToggle}>
    <FileDiff size={17} />{count > 0 && <span className="desktop-changes-count">{count}</span>}
  </button>;
}

export function DesktopChangesPanel({ language, workspace, session, query, onClose }: {
  language: Language; workspace: string; session: string;
  query: ReturnType<typeof useDesktopChanges>; onClose: () => void;
}) {
  const t = desktopText(language);
  const changes = query.data;
  const [selectedPath, selectPath] = useState<string | null>(null);
  const file = changes?.snapshot?.files.find((row) => row.path === selectedPath);
  const snapshot = changes?.snapshot?.id;
  const diff = useQuery({
    queryKey: ["desktop-diff", workspace, session, snapshot, file?.path],
    queryFn: ({ signal }) => {
      const params = changesParams(workspace, session);
      params.set("snapshot_id", snapshot ?? "");
      params.set("path", file?.path ?? "");
      return readDesktop<{ content: string }>(`${changesEndpoint}/file?${params}`, signal);
    },
    enabled: Boolean(file && !file.binary && snapshot),
  });
  return <>
    <button type="button" className="desktop-changes-backdrop" aria-label={t.close} onClick={onClose} />
    <aside id="desktop-changes" className="desktop-changes" aria-label={t.changes}
      onKeyDown={(event) => { if (event.key === "Escape") onClose(); }}>
      <header className="desktop-changes-heading">
        <h2>{t.changes}</h2>
        <button type="button" className="desktop-toolbar-button" disabled={!workspace || query.isFetching}
          title={t.refresh} aria-label={t.refresh} onClick={() => void query.refetch()}><RefreshCw size={15} /></button>
        <button type="button" className="desktop-toolbar-button" title={t.close} aria-label={t.close} onClick={onClose}><X size={17} /></button>
      </header>
      <div className="desktop-changes-body">
        {!workspace && <p className="desktop-changes-message">{t.noWorkspace}</p>}
        {workspace && query.isPending && <p role="status" className="desktop-changes-message">{t.loading}</p>}
        {query.isError && <p role="alert" className="desktop-changes-message">{t.failed}</p>}
        {changes && !changes.git && <p className="desktop-changes-message">{t.noGit}</p>}
        {changes?.snapshot?.files.length === 0 && <p className="desktop-changes-message">{t.empty}</p>}
        {changes?.snapshot?.files.length ? <ul className="desktop-changes-list">
          {changes.snapshot.files.map((row) => <li key={row.path}>
            <button type="button" aria-pressed={row.path === file?.path} title={row.old_path ? `${row.old_path} → ${row.path}` : row.path}
              onClick={() => selectPath(row.path)}>
              <span className="desktop-change-path">{row.path}</span>
              <span className="desktop-change-add">+{row.additions}</span><span className="desktop-change-remove">−{row.deletions}</span>
              {session && <small>{changes.session_paths?.includes(row.path) ? t.fromSession : changes.session_paths ? t.workspaceChange : t.attributionUnknown}</small>}
            </button>
          </li>)}
        </ul> : null}
        {changes?.snapshot?.files.length && !file ? <p className="desktop-changes-message">{t.pickFile}</p> : null}
        {file?.binary && <p className="desktop-changes-message">{t.binary}</p>}
        {file && !file.binary && diff.isPending && <p role="status" className="desktop-changes-message">{t.loading}</p>}
        {diff.isError && <p role="alert" className="desktop-changes-message">{t.failed}</p>}
        {diff.data && (diff.data.content ? <Patch content={diff.data.content} /> : <p className="desktop-changes-message">{t.noDiff}</p>)}
      </div>
    </aside>
  </>;
}

function Patch({ content }: { content: string }) {
  let offset = 0;
  return <pre className="desktop-patch">{content.split("\n").map((line) => {
    const position = offset;
    offset += line.length + 1;
    const kind = line.startsWith("@@") ? "hunk" : line.startsWith("+") && !line.startsWith("+++ ") ? "add" : line.startsWith("-") && !line.startsWith("--- ") ? "remove" : "";
    return <span className={kind} key={position}>{line}{"\n"}</span>;
  })}</pre>;
}
