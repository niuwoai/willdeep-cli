import { useQuery } from "@tanstack/react-query";

type ChangedFile = { path: string; old_path: string | null; additions: number; deletions: number; binary: boolean };
type Changes = {
  git: boolean;
  branch: string | null;
  snapshot: { id: string; files: ChangedFile[] } | null;
  session_paths: string[] | null;
};

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