// 公网入口管理面的接口层。类型与 `crates/willdeep-cli/src/web/intake.rs` 里的
// 视图一一对应：字段名照抄后端，不在这里另起别名。

export type IntakeFieldType = "text" | "textarea" | "email" | "choice";
export type IntakeSession = "continue" | "new";

export type IntakeField = {
  name: string;
  label: string;
  type: IntakeFieldType;
  required: boolean;
  options: string[];
};

export type IntakeSite = { slug: string; root: string };

export type IntakeForm = {
  slug: string;
  title: string;
  workspace: string;
  session: IntakeSession;
  prompt: string;
  fields: IntakeField[];
};

export type IntakeWebhook = {
  slug: string;
  workspace: string;
  session: IntakeSession;
  prompt: string;
  secret_env: string;
};

export type PendingRecord = { at: number; slug: string; kind: string; reason: string };
export type BindingView = { slug: string; session_id: string };

export type IntakeView = {
  enabled: boolean;
  listen: string;
  running: boolean;
  running_listen?: string | null;
  runtime_managed: boolean;
  writable: boolean;
  sites: IntakeSite[];
  forms: IntakeForm[];
  webhooks: IntakeWebhook[];
  pending: PendingRecord[];
  bindings: BindingView[];
};

export type IntakePayload = {
  enabled: boolean;
  listen: string;
  sites: IntakeSite[];
  forms: IntakeForm[];
  webhooks: IntakeWebhook[];
};

export type BrowseView = { path: string; directories: string[] };

export class IntakeFailure extends Error {
  constructor(readonly status: number, detail: string) {
    super(detail);
  }
}

async function failure(response: Response): Promise<IntakeFailure> {
  const body: unknown = await response.json().catch(() => null);
  const detail = body && typeof body === "object" && "error" in body && typeof body.error === "string" ? body.error : "";
  return new IntakeFailure(response.status, detail || response.statusText);
}

export async function loadIntake(signal?: AbortSignal): Promise<IntakeView> {
  const response = await fetch("/api/intake", { signal });
  if (!response.ok) throw await failure(response);
  return await response.json() as IntakeView;
}

export async function saveIntake(payload: IntakePayload): Promise<IntakeView> {
  const response = await fetch("/api/intake", {
    method: "PUT",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(payload),
  });
  if (!response.ok) throw await failure(response);
  return await response.json() as IntakeView;
}

export async function browseDirectory(path: string): Promise<BrowseView> {
  const query = path.trim() ? `?path=${encodeURIComponent(path.trim())}` : "";
  const response = await fetch(`/api/intake/browse${query}`);
  if (!response.ok) throw await failure(response);
  return await response.json() as BrowseView;
}

/// 站点、表单、webhook 各自的公开路径。入口服务与 Web 分开监听，所以链接用
/// 入口的实际监听地址组出来；监听在 0.0.0.0 时换成本机访问这个页面时用的主机名。
export function intakeBase(view: Pick<IntakeView, "running" | "running_listen" | "listen">): string | null {
  const address = view.running ? view.running_listen : null;
  if (!address) return null;
  const index = address.lastIndexOf(":");
  if (index <= 0) return null;
  const host = address.slice(0, index).replace(/^\[|\]$/g, "");
  const port = address.slice(index + 1);
  const name = host === "0.0.0.0" || host === "::" ? window.location.hostname : host;
  return `http://${name.includes(":") ? `[${name}]` : name}:${port}`;
}

export function intakeLink(view: IntakeView, kind: "s" | "f" | "w", slug: string): string | null {
  const base = intakeBase(view);
  return base && slug ? `${base}/${kind}/${encodeURIComponent(slug)}` : null;
}
