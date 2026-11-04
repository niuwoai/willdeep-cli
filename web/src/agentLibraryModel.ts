export async function agentLibraryCall<T>(operation: string, params: unknown = {}): Promise<T> {
  const response = await fetch(`/api/agent-library/${operation}`, {method: "POST", headers: {"Content-Type": "application/json"}, body: JSON.stringify(params)});
  const value: unknown = await response.json();
  if (!response.ok) throw new Error(typeof value === "object" && value !== null && "error" in value ? String(value.error) : String(response.status));
  return value as T;
}
export type AgentLibraryScope = {library_id: string; account: {origin: string; user_id: string; tenant_id: string} | null};

const agentWrites = new Set(["create", "update", "save", "import", "fork", "bind", "pin", "enable", "rollback", "install"]);
export function agentLibraryParams(operation: string, params: Record<string, unknown>, scope: AgentLibraryScope | undefined): Record<string, unknown> {
  return agentWrites.has(operation) ? {...params, expected_scope: scope, request_id: crypto.randomUUID()} : params;
}
export function agentRevisionConflict(error: Error | null): boolean {
  return Boolean(error && /agent_(?:revision_(?:conflict|required)|scope_conflict|request_conflict)/.test(error.message));
}
export function nextAgentVersion(version: string): string {
  const match = version.match(/^(\d+\.\d+\.\d+)(?:-rc(\d+))?$/);
  return match ? `${match[1]}-rc${Number(match[2] ?? 0) + 1}` : version;
}

