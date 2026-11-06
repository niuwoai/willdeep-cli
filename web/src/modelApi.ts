// `/model` 的数据层：先读 Provider 档案，再发现该档案的模型目录并切换 Runtime 会话。

export type ModelOption = { provider_id: string; provider: string; model: string };
export type ModelProvider = { id: string; provider: string; model: string };
export type ModelProviderCatalog = { default_provider: string; providers: ModelProvider[] };
export type ProviderModels = { models: string[]; warning: string | null };
export type ReasoningValue = string | number | boolean;
export type ReasoningProfile = { default_level: string | null; parameter: string | null; levels: Record<string, ReasoningValue> };
export type ReasoningProvider = { id: string; reasoning?: ReasoningProfile | null };
export type ReasoningProviderCatalog = { providers: ReasoningProvider[] };
export type SessionReasoningState = { reasoning_level: string | null; provider_id: string | null };

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

export async function loadModelProviders(signal?: AbortSignal): Promise<ModelProviderCatalog> {
  const response = await fetch("/api/settings/model-routing", { signal });
  if (!response.ok) throw await failure(response);
  return await response.json() as ModelProviderCatalog;
}

export async function loadProviderModels(providerId: string, signal?: AbortSignal): Promise<ProviderModels> {
  const query = new URLSearchParams({ profile: providerId });
  const response = await fetch(`/api/schedules/models?${query}`, { signal });
  if (!response.ok) throw await failure(response);
  return await response.json() as ProviderModels;
}

/// 还没建会话时这一轮会用哪个模型。服务端按 Web 所用的 Provider 档案解析，
/// 与 `/api/models` 列的是同一份配置。`writable` 说的是这个页面能不能改：
/// 只有回环监听可以，远程页面据它把选择器置灰，而不是等发出去才报 403。
export type DefaultModel = { model: ModelOption | null; writable: boolean };

export async function loadDefaultModel(signal?: AbortSignal): Promise<DefaultModel> {
  const response = await fetch("/api/models/default", { signal });
  if (!response.ok) throw await failure(response);
  return await response.json() as DefaultModel;
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

export async function loadReasoningProviders(signal?: AbortSignal): Promise<ReasoningProviderCatalog> {
  const response = await fetch("/api/settings/providers", { signal });
  if (!response.ok) throw await failure(response);
  return await response.json() as ReasoningProviderCatalog;
}

export async function loadSessionReasoningState(sessionId: string, signal?: AbortSignal): Promise<SessionReasoningState> {
  const response = await fetch(`/api/sessions/${encodeURIComponent(sessionId)}`, { signal });
  if (!response.ok) throw await failure(response);
  return await response.json() as SessionReasoningState;
}

export async function changeReasoningLevel(sessionId: string, reasoningLevel: string | null): Promise<void> {
  const response = await fetch(`/api/sessions/${encodeURIComponent(sessionId)}/reasoning-level`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ reasoning_level: reasoningLevel }),
  });
  if (response.ok) return;
  throw await failure(response);
}
