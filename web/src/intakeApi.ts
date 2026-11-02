// 公网入口管理面的接口层。类型与 `crates/willdeep-cli/src/web/intake.rs` 里的
// 视图一一对应：字段名照抄后端，不在这里另起别名。

export type IntakeFieldType = "text" | "textarea" | "email" | "tel" | "url" | "date" | "number" | "choice";
export type IntakeSession = "continue" | "new";

export type IntakeField = {
  name: string;
  label: string;
  type: IntakeFieldType;
  required: boolean;
  options: string[];
  /// 只对 `number` 有意义：是否接受小数。
  allow_decimal: boolean;
  /// 只对 `number` 有意义：允许的范围（含端点）；null 表示这一端不限。
  min: number | null;
  max: number | null;
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
  /// 访客最终访问的地址（含协议）；空串表示按监听地址拼。
  public_url: string;
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
  public_url: string;
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

/// 站点、表单和分享链接的前缀。配置了 `public_url` 就直接用它——那是访客最终
/// 访问的域名，前面通常有反向代理；没配置时退回监听地址，监听在 0.0.0.0 时换成
/// 本机访问这个页面时用的主机名。
export function intakeBase(view: Pick<IntakeView, "running" | "running_listen" | "listen" | "public_url">): string | null {
  if (!view.running) return null;
  const configured = view.public_url?.trim().replace(/\/+$/, "");
  if (configured) return configured;
  const address = view.running_listen;
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

/// 这个前缀是不是只有本机（或本机等价地址）能打开。配置了 `public_url` 也不代表
/// 它对外可达——把公网地址填成 `127.0.0.1`、`localhost` 或 `0.0.0.0` 是最容易犯的错。
export function intakeBaseIsLoopback(base: string | null): boolean {
  if (!base) return false;
  try {
    const host = new URL(base).hostname.replace(/^\[|\]$/g, "").toLowerCase();
    return host === "localhost"
      || host.endsWith(".localhost")
      || host === "::1"
      || host === "::"
      || host === "0.0.0.0"
      || host.startsWith("127.");
  } catch {
    return false;
  }
}
