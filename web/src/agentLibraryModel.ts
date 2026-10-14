export async function agentLibraryCall<T>(operation: string, params: unknown = {}): Promise<T> {
  const response = await fetch(`/api/agent-library/${operation}`, {method: "POST", headers: {"Content-Type": "application/json"}, body: JSON.stringify(params)});
  const value: unknown = await response.json();
  if (!response.ok) throw new Error(typeof value === "object" && value !== null && "error" in value ? String(value.error) : String(response.status));
  return value as T;
}
export function nextAgentVersion(version: string): string {
  const match = version.match(/^(\d+\.\d+\.\d+)(?:-rc(\d+))?$/);
  return match ? `${match[1]}-rc${Number(match[2] ?? 0) + 1}` : version;
}

