// 实例身份：这台机器叫什么。
//
// 接口只在回环 Web 上开放——远程浏览器读不到也改不了。名字是标签，id 才是身份，
// 所以改名走 `POST /api/identity/name`，返回的 id 永远不变。

export type IdentityView = {
  id: string;
  name: string;
  created_at: number;
  renamed_at?: number;
};

async function failure(response: Response): Promise<Error> {
  const body: unknown = await response.json().catch(() => null);
  const detail = body && typeof body === "object" && "error" in body && typeof body.error === "string" ? body.error : "";
  return new Error(detail || response.statusText);
}

export async function loadIdentity(signal?: AbortSignal): Promise<IdentityView> {
  const response = await fetch("/api/identity", { signal });
  if (!response.ok) throw await failure(response);
  return await response.json() as IdentityView;
}

export async function renameIdentity(name: string): Promise<IdentityView> {
  const response = await fetch("/api/identity/name", {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ name }),
  });
  if (!response.ok) throw await failure(response);
  return await response.json() as IdentityView;
}
