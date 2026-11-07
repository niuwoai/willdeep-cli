// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { ChakraProvider, defaultSystem } from "@chakra-ui/react";
import { RuntimeSidebar, type RuntimeActivity } from "./RuntimeSidebar";
import { messages, type Language } from "./i18n";
import { attentionMessages } from "./attentionMessages";

const originalScrollIntoView = HTMLElement.prototype.scrollIntoView;
afterEach(() => {
  cleanup();
  HTMLElement.prototype.scrollIntoView = originalScrollIntoView;
});

it("展开后先给运行指标，新建子 Agent 需要单独打开", () => {
  const t = messages["zh-CN"];
  const activity: RuntimeActivity = { tools: [], artifacts: [], agents: [], tasks: [], gates: [], attention_count: 1 };
  render(<ChakraProvider value={defaultSystem}><RuntimeSidebar activity={activity} events={[]} messages={t}
    onIgnoreEvent={async () => {}} onOpenGate={() => {}} onAgentAction={async () => {}} canSpawnAgent={false} onSpawnAgent={async () => false} /></ChakraProvider>);
  const toggle = screen.getByRole("button", { name: new RegExp(t.runtimeActivity) });
  expect(toggle.getAttribute("aria-expanded")).toBe("false");
  expect(screen.queryByText(t.needsAttention)).toBeNull();
  fireEvent.click(toggle);
  expect(toggle.getAttribute("aria-expanded")).toBe("true");
  expect(screen.getByText(t.needsAttention)).toBeTruthy();
  expect(screen.queryByPlaceholderText(t.agentTaskPlaceholder)).toBeNull();
  fireEvent.click(screen.getByRole("button", { name: t.newReadOnlyAgent }));
  expect(screen.getByPlaceholderText(t.agentTaskPlaceholder)).toBeTruthy();
});

it("shows one navigation reminder without duplicating question controls", () => {
  const open = vi.fn();
  const activity: RuntimeActivity = { tools: [], artifacts: [], agents: [], tasks: [], attention_count: 1,
    gates: [{ kind: "question", id: "request", task_id: "task", question: "选择方案", options: ["A", "B", "C", "D"], multi_select: false }] };
  render(<ChakraProvider value={defaultSystem}><RuntimeSidebar activity={activity} events={[]} messages={messages["zh-CN"]}
    onIgnoreEvent={async () => {}} onOpenGate={open} onAgentAction={async () => {}} canSpawnAgent={false} onSpawnAgent={async () => false} /></ChakraProvider>);
  fireEvent.click(screen.getByRole("button", { name: new RegExp(messages["zh-CN"].runtimeActivity) }));
  expect(screen.queryByRole("button", { name: "A" })).toBeNull();
  expect(screen.queryByPlaceholderText(messages["zh-CN"].otherAnswer)).toBeNull();
  fireEvent.click(screen.getByRole("button", { name: messages["zh-CN"].openHumanGate }));
  expect(open).toHaveBeenCalledExactlyOnceWith("request");
});

function fixture(status: string, workflowOwned = false) {
  const action = vi.fn(async () => {});
  const activity: RuntimeActivity = { tools: [], artifacts: [], tasks: [], gates: [], attention_count: 0,
    agents: [{ id: "child", parent_id: "root", label: "fixture", background: true, profile: "reader",
      model: null, status, current_turn: 1, current_tool: null, total_tokens: null, elapsed_seconds: 1,
      finished_seconds_ago: 0, worktree_branch: null, dedicated_worktree: false }] };
  if (workflowOwned) {
    const usage = { model_calls: 1, unknown_usage_calls: 0, accounted_tokens: 15,
      input_tokens: 10, output_tokens: 5, cache_read_tokens: null, total_tokens: 15, uncertain_execution: false };
    activity.teams = [{ parent_session_id: "parent", policy: null, running: 0, queued: 0, completed_runs: 1,
      usage, parent_usage: usage, child_usage: usage, auxiliary_usage: usage, compression_usage: usage, ledger_incomplete: false,
      workers: [{ agent_id: "child", run_count: 1, queue_position: null, cumulative_usage: usage,
        latest_run: { id: "leaf", agent_id: "child", sequence: 1, status, reason: "spawn",
          workflow_owner: { workflow_run_id: "workflow", node_instance_id: "node" },
          queued_at_ms: 1, started_at_ms: 1, finished_at_ms: 2, current_turn: 1, current_tool: null, usage, report: null } }] }];
  }
  render(<ChakraProvider value={defaultSystem}><RuntimeSidebar activity={activity} events={[]} messages={messages["zh-CN"]}
    onIgnoreEvent={async () => {}} onOpenGate={() => {}}
    onAgentAction={action} canSpawnAgent={false} onSpawnAgent={async () => false} /></ChakraProvider>);
  fireEvent.click(screen.getByRole("button", { name: new RegExp(messages["zh-CN"].runtimeActivity) }));
  return action;
}

function attentionActivity(count: number): RuntimeActivity {
  return {
    tools: [], artifacts: [], agents: [], tasks: [], gates: [], attention_count: 99,
    attention_sessions: Array.from({ length: count }, (_, index) => ({
      session_id: `session-${index}`, title: `会话标题 ${index + 1}`, active_turn_id: `turn-${index}`,
      reasons: [{ status: "waiting_answer", failure_domain: null, exit_code: null }],
    })),
  };
}

function renderAttention(activity: RuntimeActivity, language: Language = "zh-CN") {
  const open = vi.fn();
  const view = render(<ChakraProvider value={defaultSystem}><RuntimeSidebar activity={activity} events={[]}
    language={language} messages={messages[language]} onOpenSession={open}
    onIgnoreEvent={async () => {}} onOpenGate={() => {}} onAgentAction={async () => {}}
    canSpawnAgent={false} onSpawnAgent={async () => false} /></ChakraProvider>);
  return { ...view, open };
}

describe("待处理会话导航", () => {
  it("折叠数字有独立按钮，打开后滚动聚焦并完整列出全部会话", () => {
    const scroll = vi.fn();
    HTMLElement.prototype.scrollIntoView = scroll;
    const { container, open } = renderAttention(attentionActivity(7));
    const badge = screen.getByRole("button", { name: `${messages["zh-CN"].needsAttention} 7` });
    expect(container.querySelector("button button")).toBeNull();
    fireEvent.click(badge);
    const region = screen.getByRole("region", { name: attentionMessages["zh-CN"].sessions });
    expect(document.activeElement).toBe(region);
    expect(scroll).toHaveBeenCalledWith({ block: "nearest" });
    expect(within(region).getAllByRole("listitem")).toHaveLength(7);
    expect(within(region).getAllByText(attentionMessages["zh-CN"].waiting_answer)).toHaveLength(7);
    fireEvent.click(within(region).getByRole("button", { name: "会话标题 7" }));
    expect(open).toHaveBeenCalledExactlyOnceWith("session-6", "turn-6");
  });

  it("展开后的统计按钮支持键盘，重复点击仍会重新定位", async () => {
    const user = userEvent.setup();
    const scroll = vi.fn();
    HTMLElement.prototype.scrollIntoView = scroll;
    const { container } = renderAttention(attentionActivity(1));
    fireEvent.click(screen.getByRole("button", { name: messages["zh-CN"].runtimeActivity }));
    const statistic = container.querySelector<HTMLButtonElement>(".runtime-attention-stat");
    expect(statistic).not.toBeNull();
    statistic?.focus();
    await user.keyboard("{Enter}");
    const region = screen.getByRole("region", { name: attentionMessages["zh-CN"].sessions });
    expect(document.activeElement).toBe(region);
    expect(scroll).toHaveBeenCalledTimes(1);
    statistic?.focus();
    await user.keyboard(" ");
    expect(document.activeElement).toBe(region);
    expect(scroll).toHaveBeenCalledTimes(2);
  });

  it.each<Language>(["zh-CN", "en", "ja"])("%s 使用本地化原因和安全标题回退", (language) => {
    const activity = attentionActivity(1);
    activity.attention_sessions![0] = {
      session_id: "12345678-1234", title: null, active_turn_id: null,
      reasons: [{ status: "failed", failure_domain: "provider", exit_code: 23 }],
    };
    const { open } = renderAttention(activity, language);
    fireEvent.click(screen.getByRole("button", { name: `${messages[language].needsAttention} 1` }));
    const region = screen.getByRole("region", { name: attentionMessages[language].sessions });
    expect(region.textContent).toContain(attentionMessages[language].failed);
    expect(region.textContent).toContain(messages[language].failureProvider);
    expect(region.textContent).toContain(`${messages[language].exitCode}: 23`);
    fireEvent.click(within(region).getByRole("button", { name: attentionMessages[language].untitled.replace("{id}", "12345678") }));
    expect(open).toHaveBeenCalledExactlyOnceWith("12345678-1234", null);
  });

  it("旧计数或没有会话归属的失败不能伪造导航入口", () => {
    const activity = attentionActivity(0);
    activity.attention_sessions = undefined;
    const { container } = renderAttention(activity);
    expect(container.querySelector(".runtime-toggle-badge")).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: messages["zh-CN"].runtimeActivity }));
    expect(screen.getByRole("button", { name: `${messages["zh-CN"].needsAttention} 0` })).toBeTruthy();
    const region = screen.getByRole("region", { name: attentionMessages["zh-CN"].sessions });
    expect(within(region).getByText(attentionMessages["zh-CN"].empty)).toBeTruthy();
    expect(within(region).queryByRole("button")).toBeNull();
  });
});

describe("子 Agent 续做", () => {
  it.each(["working", "done", "interrupted"])("工作流节点 %s 只通过工作流提供控制", (status) => {
    fixture(status, true);
    for (const label of [messages["zh-CN"].continueAgent, messages["zh-CN"].retry, messages["zh-CN"].changeModel,
      messages["zh-CN"].instruct, messages["zh-CN"].stop]) {
      expect(screen.queryByRole("button", { name: label })).toBeNull();
    }
    expect(screen.getByRole("button", { name: messages["zh-CN"].details })).toBeTruthy();
  });
  it("完成后把续做动作交给原 Agent", () => {
    const action = fixture("done");
    fireEvent.click(screen.getByRole("button", { name: messages["zh-CN"].continueAgent }));
    expect(action).toHaveBeenCalledWith("child", "continue");
  });
  it("运行中的 Agent 只提供补充指令，不提供新的续做", () => {
    fixture("working");
    expect(screen.queryByRole("button", { name: messages["zh-CN"].continueAgent })).toBeNull();
    expect(screen.getByRole("button", { name: messages["zh-CN"].instruct })).toBeTruthy();
  });
  it("中断后的 Agent 可以显式续做", () => {
    fixture("interrupted");
    expect(screen.getByRole("button", { name: messages["zh-CN"].continueAgent })).toBeTruthy();
  });
});
