import { useEffect, useRef } from "react";
import { AtSign, Bot, FileText, Plug, Sparkles, X } from "lucide-react";
import type { DraftReference } from "./composerDraft";
import type { Messages } from "./i18n";
import type { Reference } from "./useComposerReferences";
import "./composerReferences.css";

const referenceIcons = { peer: AtSign, mcp: Plug, agent: Bot, file: FileText, skill: Sparkles };

export function ComposerReferenceChips({ items, messages: t, remove }: {
  items: DraftReference[]; messages: Messages; remove: (id: string) => void;
}) {
  if (!items.length) return null;
  const labels = { peer: t.referenceMembers, mcp: t.referenceMcp, agent: t.agents, file: t.referenceFiles, skill: t.skills };
  return <ul className="composer-reference-list" aria-label={t.references}>
    {items.map((item) => {
      const Icon = referenceIcons[item.kind];
      return <li key={item.id} className="composer-reference-chip" title={`${labels[item.kind]} · ${item.name}`}>
        <Icon size={14} aria-hidden="true" />
        <span>{item.name}</span>
        <button type="button" aria-label={t.removeReference.replace("{name}", item.name)} onClick={() => remove(item.id)}><X size={14} aria-hidden="true" /></button>
      </li>;
    })}
  </ul>;
}

export function ComposerReferences({ entries, selected, loading, messages: t, choose }: {
  entries: Reference[]; selected: number; loading: boolean; messages: Messages; choose: (entry: Reference) => void;
}) {
  const menuRef = useRef<HTMLDivElement>(null);
  useEffect(() => {
    menuRef.current?.querySelector('[aria-selected="true"]')?.scrollIntoView({ block: "nearest" });
  }, [entries, selected]);
  const labels = { peer: t.referenceMembers, mcp: t.referenceMcp, agent: t.agents, file: t.referenceFiles };
  return <div ref={menuRef} className="suggestions reference-suggestions" role="listbox" aria-label={t.references}>
    {(["peer", "mcp", "agent", "file"] as const).map((kind) => {
      const group = entries.filter((entry) => entry.kind === kind);
      return group.length > 0 && <div key={kind} role="group" aria-label={labels[kind]}>
        <div className="suggestion-title">{labels[kind]}</div>
        {group.map((entry) => <button key={entry.value} type="button" role="option" aria-selected={entries[selected]?.value === entry.value}
          onMouseDown={(event) => event.preventDefault()} onClick={() => choose(entry)}>{entry.name}</button>)}
      </div>;
    })}
    {entries.length === 0 && <div className="suggestion-title">{loading ? t.referenceLoading : t.noReferences}</div>}
  </div>;
}
