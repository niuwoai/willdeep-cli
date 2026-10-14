import { useEffect, useRef } from "react";
import type { Messages } from "./i18n";
import type { Reference } from "./useComposerReferences";
import "./composerReferences.css";

export function ComposerReferences({ entries, selected, loading, messages: t, choose }: {
  entries: Reference[]; selected: number; loading: boolean; messages: Messages; choose: (entry: Reference) => void;
}) {
  const menuRef = useRef<HTMLDivElement>(null);
  useEffect(() => {
    menuRef.current?.querySelector('[aria-selected="true"]')?.scrollIntoView({ block: "nearest" });
  }, [entries, selected]);
  const labels = { mcp: t.referenceMcp, agent: t.agents, file: t.referenceFiles };
  return <div ref={menuRef} className="suggestions reference-suggestions" role="listbox" aria-label={t.references}>
    {(["mcp", "agent", "file"] as const).map((kind) => {
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
