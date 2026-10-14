import { useEffect, useRef, useState } from "react";

export type Reference = { kind: "mcp" | "agent" | "file"; name: string; value: string };
export function referenceQuery(text: string, cursor: number) {
  const match = text.slice(0, cursor).match(/(?:^|\s)@([^\s@"]*)$/u);
  return match ? { start: cursor - match[1].length - 1, cursor, query: match[1] } : undefined;
}
export function insertReference(text: string, query: { start: number; cursor: number }, value: string) {
  return text.slice(0, query.start) + value + " " + text.slice(query.cursor);
}

const SEARCH_DELAY_MS = 150;
export function useComposerReferences(workspace: string, prompt: string, loadFailed: string, onError: (error: string) => void) {
  const [query, setQuery] = useState<(NonNullable<ReturnType<typeof referenceQuery>> & { text: string }) | undefined>();
  const [entries, setEntries] = useState<Reference[]>([]);
  const [selected, setSelected] = useState(0);
  const [loading, setLoading] = useState(false);
  const generation = useRef(0);
  const timer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  function dismiss() {
    generation.current++;
    clearTimeout(timer.current);
    setQuery(undefined);
    setEntries([]);
    setLoading(false);
  }
  useEffect(() => {
    generation.current++;
    clearTimeout(timer.current);
    setQuery(undefined);
    setEntries([]);
    setLoading(false);
    const epoch = generation; const pendingTimer = timer;
    return () => { epoch.current++; clearTimeout(pendingTimer.current); };
  }, [workspace]);
  function update(text: string, cursor: number) {
    const next = referenceQuery(text, cursor);
    const ticket = ++generation.current;
    clearTimeout(timer.current);
    setQuery(next && workspace ? { ...next, text } : undefined);
    setSelected(0);
    setEntries([]);
    setLoading(!!next && !!workspace);
    if (!next || !workspace) return;
    timer.current = setTimeout(() => {
      void fetch(`/api/composer?workspace=${encodeURIComponent(workspace)}&reference_query=${encodeURIComponent(next.query)}`)
        .then(async (response) => {
          if (!response.ok) throw new Error(String(response.status));
          return await response.json() as { references: Reference[] };
        }).then((data) => {
          if (ticket === generation.current) { setEntries(data.references); setLoading(false); }
        }).catch((error: unknown) => {
          if (ticket === generation.current) {
            setLoading(false);
            onError(`${loadFailed}: ${error instanceof Error ? error.message : String(error)}`);
          }
        });
    }, SEARCH_DELAY_MS);
  }
  return { query: query?.text === prompt ? query : undefined, entries, selected, loading, setSelected, update, dismiss };
}

