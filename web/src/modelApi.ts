// `/model` 的数据层。候选来自配置里的 Provider，切换下发到 Runtime 的会话。

export type ModelOption = { provider_id: string; provider: string; model: string };

export class ModelFailure extends Error {
  constructor(readonly status: number, detail: string) {
    super(detail);
  }
}

async function failure(response: Response): Promise<ModelFailure> {
  const body: unknown = await response.json().catch(() => null);
  const detail = body && typeof body === "object" && "error" in body && typeof body.error === "string" ? body.error : "";
  return new ModelFailure(response.status, detail || response.statusText);
}

export async function loadModels(signal?: AbortSignal): Promise<ModelOption[]> {
  const response = await fetch("/api/models", { signal });
  if (!response.ok) throw await failure(response);
  return await response.json() as ModelOption[];
}

/// `providerId` 为空时只换模型名，Provider 保持会话当前的。
export async function changeModel(sessionId: string, model: string, providerId?: string): Promise<void> {
  const response = await fetch(`/api/sessions/${encodeURIComponent(sessionId)}/model`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(providerId ? { model, provider_id: providerId } : { model }),
  });
  if (response.ok) return;
  throw await failure(response);
}
