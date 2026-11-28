// 桌面壳里安装命令行 willdeep。
//
// 命令由服务端的方案表决定：页面只把用户选中的管理器 id 交回去，拼不出表外的命令。
// macOS 有 Homebrew 就用它，没有才退到 npm、cnpm、pnpm；Linux 只有 npm；Windows 不代为执行，
// 只把命令交给用户自己跑。

import { useState } from "react";
import { useMutation, useQuery } from "@tanstack/react-query";
import type { Messages } from "./i18n";
import "./intake.css";

/// 安装接口只在桌面壳里存在。这里每次渲染时判断，而不是用模块加载时算好的值，
/// 否则测试里改 window 也来不及。
function runningInDesktop() {
  return "__willdeepDesktop" in window && window.__willdeepDesktop === true;
}

export type CliManager = { id: string; command: string };
export type ManualCommand = { label: string; command: string };
export type CliInstallPlan = { os: string; managers: CliManager[]; manual: ManualCommand[] };
export type CliInstallOutcome = { command: string; exit_code: number; output: string };

export function CliInstall({ messages: t }: { messages: Messages }) {
  const plan = useQuery({
    queryKey: ["desktop-cli-plan"],
    queryFn: ({ signal }) => readPlan(signal),
    enabled: runningInDesktop(),
    retry: false,
  });
  const [outcome, setOutcome] = useState<CliInstallOutcome | null>(null);
  const install = useMutation({
    mutationFn: (manager: string) => runInstall(manager),
    onSuccess: (value) => setOutcome(value),
  });

  return <main className="mcp-page">
    <h1>{t.cliTitle}</h1>
    <p>{t.cliIntro}</p>
    {!runningInDesktop() && <p role="alert">{t.cliDesktopOnly}</p>}
    {plan.isLoading && <p>{t.cliLoading}</p>}
    {plan.isError && <p role="alert">{t.cliLoadFailed}</p>}
    {plan.data && plan.data.managers.length === 0 && plan.data.manual.length === 0 && <p>{t.cliNone}</p>}
    {plan.data && plan.data.managers.length > 0 && <ul className="cli-options">
      {plan.data.managers.map((manager) => (
        <li key={manager.id}>
          <code>{manager.command}</code>
          <button type="button" disabled={install.isPending} onClick={() => { setOutcome(null); install.mutate(manager.id); }}>
            {install.isPending && install.variables === manager.id ? t.cliInstalling : t.cliInstall}
          </button>
        </li>
      ))}
    </ul>}
    {plan.data && plan.data.manual.length > 0 && <>
      <p>{t.cliManualHint}</p>
      <ul className="cli-options">
        {plan.data.manual.map((entry) => <li key={entry.label}><code>{entry.command}</code></li>)}
      </ul>
    </>}
    {install.isError && <p role="alert">{t.cliLoadFailed}</p>}
    {outcome && <>
      <p role="status">{outcome.exit_code === 0 ? t.cliSucceeded : t.cliFailed.replace("{code}", String(outcome.exit_code))}</p>
      {outcome.output && <pre className="mcp-snippet">{outcome.output}</pre>}
    </>}
  </main>;
}

async function readPlan(signal: AbortSignal): Promise<CliInstallPlan> {
  const response = await fetch("/api/desktop/cli", { signal });
  if (!response.ok) throw new Error(String(response.status));
  return await response.json() as CliInstallPlan;
}

async function runInstall(manager: string): Promise<CliInstallOutcome> {
  const response = await fetch("/api/desktop/cli/install", {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ manager }),
  });
  if (!response.ok) throw new Error(String(response.status));
  return await response.json() as CliInstallOutcome;
}
