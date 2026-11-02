// 实例联邦：配对、联系人和发消息。
//
// 接口只在回环 Web 上开放。联系人表里有每对实例的共享密钥，浏览器只拿到
// 展示需要的字段，密钥永远不出后端。

export type FederationContact = {
  id: string;
  short_id: string;
  name: string;
  url: string;
  session_id: string | null;
  auto_turns: number;
  added_at: number;
  last_seen: number | null;
};

export type FederationView = {
  me: { id: string; name: string };
  enabled: boolean;
  workspace: string;
  reply: "always" | "mention" | "never";
  max_auto_turns: number;
  /// 邀请链接的前缀；空串表示 [intake] public_url 还没配。
  invite_base: string;
  contacts: FederationContact[];
};

export type InviteView = { url: string; expires_at: number };

async function failure(response: Response): Promise<Error> {
  const body: unknown = await response.json().catch(() => null);
  const detail = body && typeof body === "object" && "error" in body && typeof body.error === "string" ? body.error : "";
  return new Error(detail || response.statusText);
}

export async function loadFederation(signal?: AbortSignal): Promise<FederationView> {
  const response = await fetch("/api/federation", { signal });
  if (!response.ok) throw await failure(response);
  return await response.json() as FederationView;
}

export async function invitePeer(): Promise<InviteView> {
  const response = await fetch("/api/federation/invite", { method: "POST", headers: { "content-type": "application/json" }, body: "{}" });
  if (!response.ok) throw await failure(response);
  return await response.json() as InviteView;
}

export async function joinPeer(url: string): Promise<FederationView> {
  const response = await fetch("/api/federation/join", {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ url }),
  });
  if (!response.ok) throw await failure(response);
  return await response.json() as FederationView;
}

export async function sendToPeer(id: string, text: string): Promise<void> {
  const response = await fetch(`/api/federation/${encodeURIComponent(id)}/message`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ text }),
  });
  if (!response.ok) throw await failure(response);
}

export async function removePeer(id: string): Promise<FederationView> {
  const response = await fetch(`/api/federation/${encodeURIComponent(id)}`, { method: "DELETE" });
  if (!response.ok) throw await failure(response);
  return await response.json() as FederationView;
}
