import { useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { FileDiff, RefreshCw, X } from "lucide-react";
import type { Language } from "./i18n";
import { desktopText } from "./desktopMessages";
import { readDesktop, staleChanges, useDesktopChanges, useWorkspaceFileDiff, type ChangedFile, type DiffArea } from "./useDesktopChanges";
import "./desktopWorkspace.css";

type RuntimeVersion = { desktop_version: string; runtime_version: string; protocol_version: string };

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
  const [selection, setSelection] = useState<{ path: string; area: DiffArea } | null>(null);
  const files = changes?.snapshot?.files ?? [];
  const attributedPaths = new Set(changes?.session_paths);
  const sessionFiles = session ? files.filter((row) => attributedPaths.has(row.path)) : [];
  const workspaceFiles = session ? files.filter((row) => !attributedPaths.has(row.path)) : files;
  const file = files.find((row) => row.path === selection?.path);
  const area = file?.staged && (!file.unstaged || selection?.area === "staged") ? "staged" : "unstaged";
  const binary = file?.binary && !(file.staged && file.unstaged);
  const diff = useWorkspaceFileDiff(workspace, session, changes?.snapshot?.id, binary ? undefined : file?.path, area);
  const selectFile = (row: ChangedFile) => setSelection({ path: row.path, area: row.staged ? "staged" : "unstaged" });
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
        {sessionFiles.length > 0 && <ChangesList title={t.fromSession} rows={sessionFiles} selectedPath={file?.path} onSelect={selectFile} />}
        {workspaceFiles.length > 0 && <ChangesList title={session ? t.otherChanges : undefined} rows={workspaceFiles} selectedPath={file?.path} onSelect={selectFile} />}
        {files.length > 0 && !file && <p className="desktop-changes-message">{t.pickFile}</p>}
      </div>
      {file && <section className="desktop-change-detail" aria-label={t.fileDiff}>
        <h3 title={file.path}>{file.path}</h3>
        {sessionFiles.some((row) => row.path === file.path) && <p className="desktop-change-scope">{t.attributionScope}</p>}
        {file.staged && file.unstaged ? <div className="desktop-change-areas" role="group" aria-label={t.fileDiff}>
          <button type="button" aria-pressed={area === "staged"} onClick={() => setSelection({ path: file.path, area: "staged" })}>{t.staged}</button>
          <button type="button" aria-pressed={area === "unstaged"} onClick={() => setSelection({ path: file.path, area: "unstaged" })}>{t.unstaged}</button>
        </div> : <p className="desktop-change-scope">{file.kind === "untracked" ? t.untracked : area === "staged" ? t.staged : t.unstaged}</p>}
        {binary && <p className="desktop-changes-message">{t.binary}</p>}
        {!binary && diff.isPending && <p role="status" className="desktop-changes-message">{t.loading}</p>}
        {diff.isError ? <p role="alert" className="desktop-changes-message">{staleChanges(diff.error) ? t.staleSnapshot : t.failed}</p>
          : diff.data && (diff.data.content ? <Patch content={diff.data.content} /> : <p className="desktop-changes-message">{t.noDiff}</p>)}
      </section>}
    </aside>
  </>;
}

function ChangesList({ title, rows, selectedPath, onSelect }: {
  title?: string; rows: ChangedFile[]; selectedPath?: string; onSelect: (row: ChangedFile) => void;
}) {
  return <section aria-label={title}>
    {title && <h3 className="desktop-changes-group">{title}</h3>}
    <ul className="desktop-changes-list">
      {rows.map((row) => <li key={row.path}>
        <button type="button" aria-pressed={row.path === selectedPath} title={row.old_path ? `${row.old_path} → ${row.path}` : row.path}
          onClick={() => onSelect(row)}>
          <span className="desktop-change-path">{row.path}</span>
          <span className="desktop-change-add">+{row.additions}</span><span className="desktop-change-remove">−{row.deletions}</span>
        </button>
      </li>)}
    </ul>
  </section>;
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
