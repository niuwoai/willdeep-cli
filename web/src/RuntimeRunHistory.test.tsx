// @vitest-environment jsdom
import { afterEach, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { ChakraProvider, defaultSystem } from "@chakra-ui/react";
import { RuntimeRunHistory } from "./RuntimeRunHistory";
import { messages } from "./i18n";
import type { AgentRun } from "./RuntimeSidebar";

afterEach(cleanup);
const t = messages["zh-CN"];
const run: AgentRun = { id: "run-one", agent_id: "child", sequence: 1, status: "blocked", reason: "continue",
  queued_at_ms: 1, started_at_ms: 2, finished_at_ms: 3, current_turn: 1, current_tool: null, report: "budget exhausted",
  usage: { model_calls: 1, unknown_usage_calls: 0, accounted_tokens: 15, input_tokens: 10, output_tokens: 5, total_tokens: 15, cache_read_tokens: 2, uncertain_execution: false } };
function view(load: (id: string, after?: number) => Promise<{ runs: AgentRun[]; next_after_sequence: number | null }>) {
  render(<ChakraProvider value={defaultSystem}><RuntimeRunHistory id="child" onLoad={load} messages={t} /></ChakraProvider>);
}
it("显式查看报告并按游标加载下一页，重复项只显示一次", async () => {
  const load = vi.fn().mockResolvedValueOnce({ runs: [run], next_after_sequence: 1 }).mockResolvedValueOnce({ runs: [run, { ...run, id: "run-two", sequence: 2 }], next_after_sequence: null });
  view(load);
  expect(load).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole("button", { name: t.runHistory }));
  await screen.findByText("budget exhausted");
  fireEvent.click(screen.getByRole("button", { name: t.moreRuns }));
  await waitFor(() => expect(screen.getAllByText("budget exhausted")).toHaveLength(2));
  expect(load).toHaveBeenNthCalledWith(2, "child", 1);
  expect(screen.queryByRole("button", { name: t.moreRuns })).toBeNull();
});
it("历史加载失败时显示错误并保留重试入口", async () => {
  const load = vi.fn().mockRejectedValueOnce(new Error("offline")).mockResolvedValueOnce({ runs: [], next_after_sequence: null });
  view(load);
  fireEvent.click(screen.getByRole("button", { name: t.runHistory }));
  await screen.findByRole("alert");
  fireEvent.click(screen.getByRole("button", { name: t.runHistory }));
  await screen.findByText(t.noRunHistory);
});
