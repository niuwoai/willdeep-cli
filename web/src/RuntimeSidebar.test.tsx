// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { ChakraProvider, defaultSystem } from "@chakra-ui/react";
import { RuntimeSidebar, type RuntimeActivity } from "./RuntimeSidebar";
import { messages } from "./i18n";

afterEach(cleanup);

function fixture(status: string) {
  const action = vi.fn(async () => {});
  const activity: RuntimeActivity = { tools: [], artifacts: [], tasks: [], gates: [], attention_count: 0,
    agents: [{ id: "child", parent_id: "root", label: "fixture", background: true, profile: "reader",
      model: null, status, current_turn: 1, current_tool: null, total_tokens: null, elapsed_seconds: 1,
      finished_seconds_ago: 0, worktree_branch: null, dedicated_worktree: false }] };
  render(<ChakraProvider value={defaultSystem}><RuntimeSidebar activity={activity} events={[]} messages={messages["zh-CN"]}
    onIgnoreEvent={async () => {}} onResolveApproval={async () => {}} onAnswerQuestion={async () => {}}
    onAgentAction={action} canSpawnAgent={false} onSpawnAgent={async () => false} /></ChakraProvider>);
  fireEvent.click(screen.getByRole("button", { name: messages["zh-CN"].expand }));
  return action;
}

describe("子 Agent 续做", () => {
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
