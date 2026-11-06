import { useCallback, useRef, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import type { WorkspaceAccess } from "./accessModes";
import { addWorkspacePath, listWorkspaces, saveWorkspaceApproval, workspaceListKey, type WorkspaceSummary } from "./workspaceApprovalApi";

export type WorkspaceEntry = { session?: { id: string; turnId: string | null }; fresh?: boolean; added?: boolean; approvalMode?: WorkspaceAccess };
type PendingEntry = { workspace: WorkspaceSummary; entry: WorkspaceEntry; revision: number };

export function needsWorkspaceApproval(workspace: WorkspaceSummary): boolean {
  // 旧服务端/旧记录缺少标记时保留原选择；只读策略从不通过引导升档。
  return workspace.approval_mode_configured === false && workspace.access !== "read_only";
}

/** 目录选择是一笔可取消的事务：保存成功之前，宿主保留原目录、会话和输入。 */
export function useWorkspaceApproval(onEntered: (workspace: WorkspaceSummary, entry: WorkspaceEntry) => void) {
  const client = useQueryClient();
  const query = useQuery({ queryKey: workspaceListKey, queryFn: listWorkspaces });
  const [pending, setPending] = useState<PendingEntry | null>(null);
  const revision = useRef(0);
  const entered = useRef(onEntered);
  entered.current = onEntered;
  const remember = useCallback((workspace: WorkspaceSummary) => {
    client.setQueryData<WorkspaceSummary[]>(workspaceListKey, (current = []) => {
      const found = current.some((item) => item.path === workspace.path);
      return found ? current.map((item) => item.path === workspace.path ? workspace : item) : [...current, workspace];
    });
  }, [client]);
  const add = useMutation({ mutationFn: addWorkspacePath, onSuccess: remember });
  const save = useMutation({
    mutationFn: ({ path, mode, confirmed }: { path: string; mode: WorkspaceAccess; confirmed: boolean }) => saveWorkspaceApproval(path, mode, confirmed),
    onMutate: () => client.cancelQueries({ queryKey: workspaceListKey }),
  });

  const requestEntry = useCallback((workspace: WorkspaceSummary, entry: WorkspaceEntry = {}) => {
    const nextRevision = ++revision.current;
    if (needsWorkspaceApproval(workspace)) setPending({ workspace, entry, revision: nextRevision });
    else { setPending(null); entered.current(workspace, entry); }
  }, []);
  const cancel = useCallback(() => { ++revision.current; setPending(null); }, []);

  async function apply(mode: WorkspaceAccess, confirmed: boolean) {
    if (!pending || mode === "read_only" || mode === "full_access" && !confirmed) return;
    const selection = pending;
    const saved = await save.mutateAsync({ path: selection.workspace.path, mode, confirmed });
    // 用户已取消或改选另一个目录时，迟到的响应不能切目录或改当前会话。
    if (selection.revision !== revision.current) return;
    if (saved.path !== selection.workspace.path) throw new Error();
    remember(saved);
    ++revision.current;
    setPending(null);
    entered.current(saved, selection.entry);
  }

  return { workspaces: query.data ?? [], query, pending, requestEntry, cancel, apply, addWorkspace: add.mutateAsync };
}
