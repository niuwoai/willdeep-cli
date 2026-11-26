import { useQuery } from "@tanstack/react-query";

export type ChangedFile = {
  path: string;
  old_path: string | null;
  kind: "added" | "modified" | "deleted" | "renamed" | "copied" | "unmerged" | "untracked";
  staged: boolean;
  unstaged: boolean;
  additions: number;
  deletions: number;
  binary: boolean;
};
type Changes = {
  git: boolean;
  branch: string | null;
  snapshot: { id: string; files: ChangedFile[] } | null;
  session_paths: string[] | null;
};

const changesEndpoint = "/api/workspaces/changes";
export type DiffArea = "staged" | "unstaged";

class ChangesRequestError extends Error {
  constructor(readonly status: number) { super(String(status)); }
}

export function staleChanges(error: unknown) {
  return error instanceof ChangesRequestError && error.status === 409;
}

export async function readDesktop<T>(path: string, signal: AbortSignal): Promise<T> {
  const response = await fetch(path, { signal });
  if (!response.ok) throw new ChangesRequestError(response.status);
  return await response.json() as T;
}

export function useWorkspaceFileDiff(workspace: string, session: string, snapshot: string | undefined, path: string | undefined, area: DiffArea | undefined) {
  return useQuery({
    queryKey: ["workspace-file-diff", workspace, session, snapshot, path, area],
    queryFn: ({ signal }) => {
      const params = changesParams(workspace, session);
      params.set("snapshot_id", snapshot ?? "");
      params.set("path", path ?? "");
      params.set("area", area ?? "");
      return readDesktop<{ content: string }>(`${changesEndpoint}/file?${params}`, signal);
    },
    enabled: Boolean(snapshot && path && area),
    retry: false,
  });
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