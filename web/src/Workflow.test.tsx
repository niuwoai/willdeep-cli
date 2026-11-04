// @vitest-environment jsdom
import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { QueryClient, QueryClientProvider, focusManager, onlineManager } from "@tanstack/react-query";
import { ChakraProvider, defaultSystem } from "@chakra-ui/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { messages, type Language } from "./i18n";
import { WorkflowCard } from "./WorkflowCard";
import { WorkflowRunView } from "./WorkflowRunView";
import { WorkflowSidebar } from "./WorkflowSidebar";
import { RuntimeSidebar, type AgentUsageSummary } from "./RuntimeSidebar";
import { cacheWorkflow, loadWorkflowSchema, runWorkflow, useWorkflowList, workflowCall, WorkflowApiError, workflowFromEvent,
  workflowKeys, WORKFLOW_POLL_MS, type WorkflowRun, type WorkflowNode, type WorkflowNodeInstance, type WorkflowAgentRun } from "./workflowApi";
import { workflowText } from "./workflowMessages";

const PARENT = "00000000-0000-4000-8000-000000000001";
const RUN = "00000000-0000-4000-8000-000000000002";
const NEXT = "00000000-0000-4000-8000-000000000003";
const START = Date.UTC(2026, 10, 4, 0, 0, 0);
const emptyUsage: AgentUsageSummary = { model_calls: 1, unknown_usage_calls: 1, accounted_tokens: 0,
  input_tokens: null, output_tokens: null, cache_read_tokens: null, total_tokens: null, uncertain_execution: true };
function agent(id: string, phase: string | null = null, depends_on: string[] = []): WorkflowNode {
  return { kind: "agent", id, depends_on, phase, profile: "generalist", prompt: "Inspect input", input: null,
    output_schema: null, worker_tier: null, max_turns: null, token_budget: null, timeout_seconds: null, write_targets: [], task_packet: null };
}
function agentRun(overrides: Partial<WorkflowAgentRun> = {}): WorkflowAgentRun {
  return { id: "actual-agent-run", agent_id: "worker", parent_session_id: PARENT, request_id: "dispatch-request", sequence: 1,
    reason: "spawn", status: "running", label: "Inspect input", model: "actual-provider-model", queued_at_ms: START,
    started_at_ms: START + 1000, finished_at_ms: START + 3500, max_turns: null, token_budget: null, timeout_seconds: null,
    current_turn: 1, current_tool: null, usage: emptyUsage, report: null, delivered: false, ...overrides };
}
function node(path: string, overrides: Partial<WorkflowNodeInstance> = {}): WorkflowNodeInstance {
  return { id: `instance-${path}`, node_id: path.split("/").at(-1)!, path, attempt_id: `attempt-${path}`,
    status: "running", request_id: "dispatch-request", agent_run: agentRun(), output: null, report: null, error: null,
    usage: emptyUsage, created_at_ms: START, started_at_ms: START + 1000, finished_at_ms: null, reused_from: null, ...overrides };
}
function fixture(overrides: Partial<WorkflowRun> = {}): WorkflowRun {
  return { id: RUN, parent_session_id: PARENT, definition_id: "pipeline", status: "running",
    spec: { version: 1, id: "pipeline", name: "Pipeline review", input_schema: null,
      phases: [{ id: "prepare", name: "Preparation" }, { id: "review", name: "Review" }],
      nodes: [agent("collect", "prepare"), agent("review", "review", ["collect"])], output: null },
    input: { target: "fixture" }, nodes: [node("collect"), node("review")],
    limits: { max_definition_bytes: 1048576, max_run_bytes: 33554432, max_nodes: 256, max_depth: 16,
      max_instances: 4096, max_attempts: 4096, max_foreach_items: 256, max_iterations: 128, max_concurrent: 16,
      max_value_bytes: 1048576, max_report_bytes: 65536, max_name_bytes: 128 },
    output: null, report: null, error: null, created_at_ms: START, finished_at_ms: null, source_run_id: null, acknowledged: false,
    ...overrides };
}
type Call = { operation: string; params: Record<string, unknown> };
function mockApi(handler: (call: Call) => unknown | Promise<unknown>) {
  return vi.stubGlobal("fetch", vi.fn(async (url: string | URL | Request, init?: RequestInit) => {
    const operation = String(url).split("/").at(-1)!;
    const params = JSON.parse(String(init?.body)) as Record<string, unknown>;
    const body = await handler({ operation, params });
    return new Response(JSON.stringify(body), { status: 200, headers: { "content-type": "application/json" } });
  }));
}
const clients: QueryClient[] = [];
function client() {
  const result = new QueryClient({ defaultOptions: { queries: { retry: false, staleTime: 0, gcTime: Infinity }, mutations: { retry: false } } });
  clients.push(result); return result;
}
function renderCard(run: WorkflowRun, language: Language = "en", canControl = true) {
  const queryClient = client();
  return { queryClient, ...render(<QueryClientProvider client={queryClient}><WorkflowCard workflow={run} language={language} messages={messages[language]} canControl={canControl} initiallyExpanded /></QueryClientProvider>) };
}
afterEach(() => { cleanup(); clients.forEach((item) => item.clear()); clients.length = 0;
  vi.useRealTimers(); vi.unstubAllGlobals(); vi.restoreAllMocks(); onlineManager.setOnline(true); focusManager.setFocused(undefined); });

it("uses scoped POST requests, UUID control receipts, schema and run input without token storage", async () => {
  const calls: Call[] = [];
  mockApi((call) => { calls.push(call); return call.operation === "schema" ? { schema: {}, example: fixture().spec } : { workflow: fixture() }; });
  await loadWorkflowSchema(PARENT);
  await runWorkflow(PARENT, "pipeline", { a: 1 });
  const request = vi.mocked(fetch).mock.calls[0];
  expect(request[0]).toBe("/api/workflows/schema");
  expect(request[1]).toMatchObject({ method: "POST", headers: { "content-type": "application/json" } });
  expect(calls.every((call) => call.params.parent_session_id === PARENT)).toBe(true);
  expect(calls[1].params).toMatchObject({ definition_id: "pipeline", input: { a: 1 } });
  expect(calls[1].params.request_id).toMatch(/^[0-9a-f-]{36}$/);
});
it.each(["legacy", "control"])("retains %s API errors and status", async (style) => {
  vi.stubGlobal("fetch", vi.fn(async () => new Response(JSON.stringify({ error: style === "control" ? { code: "conflict", message: "Run already active" } : "Run already active" }), { status: 409 })));
  await expect(workflowCall("retry", PARENT)).rejects.toMatchObject({ status: 409, message: "Run already active" });
});
it("handles a non-JSON permission response", async () => {
  vi.stubGlobal("fetch", vi.fn(async () => new Response("Denied", { status: 403, statusText: "Forbidden" })));
  await expect(workflowCall("list", PARENT)).rejects.toBeInstanceOf(WorkflowApiError);
});

it("keeps declared phase order while displaying cross-phase pipeline concurrency", () => {
  const run = fixture();
  run.spec.nodes = [agent("collect-a", "prepare"), agent("collect-b", "prepare"), agent("review-a", "review", ["collect-a"])];
  run.nodes = [node("collect-a", { status: "completed" }), node("collect-b"), node("review-a")];
  const { container } = render(<WorkflowRunView run={run} language="en" messages={messages.en} />);
  expect([...container.querySelectorAll(".workflow-phase > summary")].map((item) => item.textContent)).toEqual(["Preparation", "Review"]);
  expect(container.querySelectorAll('.workflow-status[data-status="running"]')).toHaveLength(2);
  expect(container.querySelector('[data-node-path="review-a"] .workflow-status')?.textContent).toBe("Running");
  expect(screen.getByText(workflowText("en").phasesHint)).toBeTruthy();
  expect(screen.getByText("Depends on: collect-a")).toBeTruthy();
});
it("shows actual AgentRun model, timestamps and duration without filling missing usage with zero", () => {
  const { container } = render(<WorkflowRunView run={fixture({ nodes: [node("collect")] })} language="en" messages={messages.en} />);
  const facts = container.querySelector('[data-node-path="collect"]')!;
  expect(within(facts as HTMLElement).getByText("actual-provider-model")).toBeTruthy();
  expect(within(facts as HTMLElement).getByText("2.5 s")).toBeTruthy();
  const usage = within(facts as HTMLElement).getByLabelText("Usage");
  expect([...usage.querySelectorAll("dd")].map((item) => item.textContent)).toEqual(["Unknown", "Unknown", "Unknown", "Unknown", "0"]);
  expect(screen.getByText("Calls with unknown usage: 1")).toBeTruthy();
  expect(screen.getByText("Execution usage is uncertain")).toBeTruthy();
  expect(facts.querySelectorAll("time")).toHaveLength(2);
});
it("preserves report, output, errors, attempt and reuse provenance", () => {
  const run = fixture({ source_run_id: "original-run", report: "Final findings", output: { accepted: true }, error: "Node failed",
    nodes: [node("collect", { status: "completed", report: "Collected findings", output: { files: 3 }, reused_from: { run_id: "reused-run", node_instance_id: "original-node" } })] });
  render(<WorkflowRunView run={run} language="en" messages={messages.en} />);
  for (const text of ["Final findings", "Collected findings", "original-run", "reused-run", "original-node", "attempt-collect", "Error: Node failed"])
    expect(screen.getByText(text)).toBeTruthy();
  expect(screen.getByText(/"accepted": true/)).toBeTruthy();
  expect(screen.getByText(/"files": 3/)).toBeTruthy();
});
it("expands foreach and repeat instances while preserving their true paths", () => {
  const run = fixture();
  run.spec.nodes = [{ kind: "foreach", id: "batch", phase: "prepare", depends_on: [], items: [1, 2], body: { nodes: [agent("inspect")], output: null } },
    { kind: "repeat", id: "refine", phase: "review", depends_on: [], initial: null, body: { nodes: [agent("improve")], output: null }, until: { op: "eq", left: 1, right: 1 }, carry: null, max_iterations: 3 }];
  run.nodes = [node("batch", { agent_run: null }), node("batch/0/inspect"), node("batch/1/inspect"), node("refine", { agent_run: null }), node("refine/0/improve")];
  const { container } = render(<WorkflowRunView run={run} language="en" messages={messages.en} />);
  const batch = container.querySelector('[data-node-path="batch"] > details') as HTMLDetailsElement;
  expect(batch.open).toBe(false);
  fireEvent.click(batch.querySelector("summary")!);
  expect(batch.open).toBe(true);
  const iteration = screen.getByText("Item 2"); fireEvent.click(iteration);
  expect((iteration.parentElement as HTMLDetailsElement).open).toBe(true);
  expect(container.querySelector('[data-node-path="batch/1/inspect"]')).toBeTruthy();
  expect(screen.getByText("Iteration 1")).toBeTruthy();
});

it("renders a saved conversation snapshot immediately and refreshes it to terminal state", async () => {
  const terminal = fixture({ status: "completed", report: "Recovered final report", finished_at_ms: START + 5000 });
  let complete!: (value: unknown) => void;
  mockApi(() => new Promise((resolve) => { complete = resolve; }));
  renderCard(fixture());
  expect(screen.getByText("Pipeline review")).toBeTruthy();
  expect(screen.getAllByText("Running").length).toBeGreaterThan(0);
  await act(async () => complete({ workflow: terminal }));
  expect(await screen.findByText("Recovered final report")).toBeTruthy();
  expect(screen.queryByRole("button", { name: "Stop workflow" })).toBeNull();
});
it("refreshes terminal replay snapshots once as well", async () => {
  const snapshot = fixture({ status: "completed", report: "Saved report" });
  mockApi(() => ({ workflow: snapshot }));
  renderCard(snapshot);
  expect(screen.getByText("Saved report")).toBeTruthy();
  await waitFor(() => expect(fetch).toHaveBeenCalledTimes(1));
});
it("sends stop to the true run ID and retains stopping until the server confirms completion", async () => {
  const calls: Call[] = [];
  let stop!: (value: unknown) => void;
  mockApi((call) => { calls.push(call); return call.operation === "stop" ? new Promise((resolve) => { stop = resolve; }) : { workflow: fixture() }; });
  renderCard(fixture());
  await waitFor(() => expect(fetch).toHaveBeenCalledTimes(1));
  fireEvent.click(screen.getByRole("button", { name: "Stop workflow" }));
  await waitFor(() => expect(calls.find((call) => call.operation === "stop")).toBeTruthy());
  expect(calls[1].params).toMatchObject({ run_id: RUN, parent_session_id: PARENT });
  expect(calls[1].params.request_id).toMatch(/^[0-9a-f-]{36}$/);
  expect(screen.getByRole("status").textContent).toBe("Submitting…");
  await act(async () => stop({ workflow: fixture({ status: "stopping" }) }));
  await waitFor(() => expect(screen.getByRole("status").textContent).toContain("Stopping"));
  expect((screen.getByRole("button", { name: "Stop workflow" }) as HTMLButtonElement).disabled).toBe(true);
  expect(screen.queryByText("Completed")).toBeNull();
});
it.each(["retry", "resume"] as const)("uses the %s receipt's new run ID and preserves the source run", async (action) => {
  const source = fixture({ status: action === "retry" ? "failed" : "interrupted" });
  const next = fixture({ id: NEXT, source_run_id: RUN, created_at_ms: START + 10000 });
  const calls: Call[] = [];
  mockApi((call) => { calls.push(call); return { workflow: call.operation === action || call.params.run_id === NEXT ? next : source }; });
  const { queryClient, container } = renderCard(source);
  fireEvent.click(screen.getByRole("button", { name: action === "retry" ? "Retry failed nodes" : "Resume interrupted run" }));
  await waitFor(() => expect(container.querySelector(`[data-workflow-id="${NEXT}"]`)).toBeTruthy());
  const nextCard = container.querySelector(`[data-workflow-id="${NEXT}"]`)!;
  fireEvent.click(within(nextCard as HTMLElement).getByRole("button", { name: "Stop workflow" }));
  await waitFor(() => expect(calls.some((call) => call.operation === "stop" && call.params.run_id === NEXT)).toBe(true));
  expect(queryClient.getQueryData<WorkflowRun>(workflowKeys.run(PARENT, RUN))?.status).toBe(source.status);
});
it("surfaces a rejected control receipt without faking a successful state change", async () => {
  vi.stubGlobal("fetch", vi.fn(async (url: string) => new Response(JSON.stringify(url.endsWith("/stop") ? { error: { code: "conflict", message: "Run state changed" } } : { workflow: fixture() }), { status: url.endsWith("/stop") ? 409 : 200 })));
  renderCard(fixture());
  fireEvent.click(screen.getByRole("button", { name: "Stop workflow" }));
  expect(await screen.findByRole("alert")).toHaveProperty("textContent", "Workflow request failed: Run state changed");
  expect((screen.getByRole("button", { name: "Stop workflow" }) as HTMLButtonElement).disabled).toBe(false);
});
it("disables writes for a read-only connection but retains inspection", async () => {
  mockApi(() => ({ workflow: fixture() }));
  renderCard(fixture(), "en", false);
  expect((screen.getByRole("button", { name: "Stop workflow" }) as HTMLButtonElement).disabled).toBe(true);
  expect(screen.getByText(workflowText("en").readOnly)).toBeTruthy();
  expect(screen.getByRole("button", { name: "Refresh" })).toBeTruthy();
});
it("loads history scoped to its definition and refreshes the selected historical run", async () => {
  const old = fixture({ id: "historical-run", status: "completed", report: "Historical report", created_at_ms: START - 10000 });
  const calls: Call[] = [];
  mockApi((call) => { calls.push(call); return call.operation === "history" ? { runs: [old] } : { workflow: call.params.run_id === old.id ? old : fixture() }; });
  const { container } = renderCard(fixture());
  fireEvent.click(screen.getByRole("button", { name: "Run history" }));
  fireEvent.click(await screen.findByRole("button", { name: /historical-run/ }));
  expect(await screen.findByText("Historical report")).toBeTruthy();
  await waitFor(() => expect(calls.some((call) => call.operation === "get" && call.params.run_id === old.id)).toBe(true));
  expect(calls.find((call) => call.operation === "history")?.params).toEqual({ parent_session_id: PARENT, definition_id: "pipeline" });
  expect(container.querySelector(`[data-workflow-id="${RUN}"]`)).toBeTruthy();
});

it.each(["zh-CN", "en", "ja"] as const)("localizes workflow controls, statuses and unknown usage in %s", async (language) => {
  mockApi(() => ({ workflow: fixture() }));
  renderCard(fixture(), language);
  const t = workflowText(language);
  expect(screen.getByRole("button", { name: t.stop })).toBeTruthy();
  expect(screen.getByRole("button", { name: t.history })).toBeTruthy();
  expect(screen.getAllByText(t.statuses.running).length).toBeGreaterThan(0);
  expect(screen.getAllByText(t.unknown).length).toBeGreaterThan(0);
  expect(screen.getByText(t.phasesHint)).toBeTruthy();
});
it("keeps workflows alongside regular workers and uses the selected parent session", async () => {
  const run = fixture();
  const calls: Call[] = [];
  mockApi((call) => { calls.push(call); return call.operation === "list" ? { definitions: [run.spec], runs: [run, fixture({ id: "foreign", parent_session_id: "other-parent" })] } : { workflow: run }; });
  const queryClient = client();
  render(<QueryClientProvider client={queryClient}><ChakraProvider value={defaultSystem}><RuntimeSidebar activity={{ tools: [], artifacts: [], tasks: [], gates: [], attention_count: 0,
    agents: [{ id: "ordinary", parent_id: null, label: "Ordinary worker", background: true, profile: "generalist", model: "worker-model", status: "working", current_turn: 1, current_tool: null, total_tokens: null, elapsed_seconds: 1, finished_seconds_ago: null, worktree_branch: null, dedicated_worktree: false }] }} events={[]} messages={messages.en}
    onIgnoreEvent={async () => {}} onOpenGate={() => {}} onAgentAction={async () => {}} canSpawnAgent={false} onSpawnAgent={async () => false}
    workflowPanel={<WorkflowSidebar parentSessionId={PARENT} language="en" messages={messages.en} canControl />} /></ChakraProvider></QueryClientProvider>);
  fireEvent.click(screen.getByRole("button", { name: /Runtime activity/ }));
  expect(screen.getByText("Ordinary worker")).toBeTruthy();
  fireEvent.click(screen.getByRole("button", { name: /Workflows/ }));
  expect(await screen.findByText("Pipeline review")).toBeTruthy();
  expect(calls.find((call) => call.operation === "list")?.params).toEqual({ parent_session_id: PARENT });
  expect(document.querySelector('[data-workflow-id="foreign"]')).toBeNull();
});
it("shows list permission failures with a retry path", async () => {
  vi.stubGlobal("fetch", vi.fn(async () => new Response(JSON.stringify({ error: "loopback required" }), { status: 403 })));
  render(<QueryClientProvider client={client()}><WorkflowSidebar parentSessionId={PARENT} language="en" messages={messages.en} canControl={false} /></QueryClientProvider>);
  fireEvent.click(screen.getByRole("button", { name: /Workflows/ }));
  expect(await screen.findByRole("alert")).toHaveProperty("textContent", "Workflows are only available through a local connection.: loopback required");
  expect(screen.getByRole("button", { name: "Refresh" })).toBeTruthy();
});

it("adapts SSE envelopes and updates the same run across list, details and history caches", async () => {
  const queryClient = client();
  const run = fixture();
  queryClient.setQueryData(workflowKeys.list(PARENT), { definitions: [run.spec], runs: [run] });
  queryClient.setQueryData(workflowKeys.history(PARENT, "pipeline"), { runs: [run] });
  const terminal = fixture({ status: "completed" });
  for (const event of [{ type: "workflow_updated", workflow: terminal }, { type: "workflow_updated", data: { workflow: terminal } }, { kind: "workflow_updated", data: { workflow: terminal } }]) {
    expect(workflowFromEvent(event)).toEqual(terminal);
    await cacheWorkflow(queryClient, workflowFromEvent(event)!);
  }
  expect(queryClient.getQueryData<WorkflowRun>(workflowKeys.run(PARENT, RUN))?.status).toBe("completed");
  expect(queryClient.getQueryData<{ runs: WorkflowRun[] }>(workflowKeys.history(PARENT, "pipeline"))?.runs).toEqual([terminal]);
  expect(queryClient.getQueryData<{ definitions: unknown[]; runs: WorkflowRun[] }>(workflowKeys.list(PARENT))).toEqual({ definitions: [run.spec], runs: [terminal] });
  expect(workflowFromEvent({ kind: "workflow_updated", data: { workflow: { id: "invalid" } } })).toBeNull();
});

it("prevents a stale in-flight get from overwriting a terminal event", async () => {
  const queryClient = client();
  let finish!: (run: WorkflowRun) => void;
  const pending = queryClient.fetchQuery({ queryKey: workflowKeys.run(PARENT, RUN),
    queryFn: () => new Promise<WorkflowRun>((resolve) => { finish = resolve; }) }).catch(() => undefined);
  const terminal = fixture({ status: "completed", report: "Terminal event" });
  await cacheWorkflow(queryClient, terminal);
  finish(fixture());
  await pending;
  expect(queryClient.getQueryData(workflowKeys.run(PARENT, RUN))).toEqual(terminal);
});

it("keeps newer running node progress when an older query receipt arrives", () => {
  const queryClient = client();
  const fresh = fixture({ revision: 9, nodes: [node("collect", { status: "running" })] });
  cacheWorkflow(queryClient, fresh);
  const stale = fixture({ revision: 8, nodes: [node("collect", { status: "pending" })] });
  expect(cacheWorkflow(queryClient, stale, workflowKeys.run(PARENT, RUN))).toEqual(fresh);
  expect(queryClient.getQueryData(workflowKeys.run(PARENT, RUN))).toEqual(fresh);
});

describe("reconnection and bounded polling", () => {
  function ListProbe() { const query = useWorkflowList(PARENT); return <span>{query.data?.runs[0]?.status}</span>; }
  it("polls only active runs and stops after a terminal receipt", async () => {
    vi.useFakeTimers();
    let current = fixture();
    mockApi(() => ({ definitions: [], runs: [current] }));
    render(<QueryClientProvider client={client()}><ListProbe /></QueryClientProvider>);
    await act(async () => { await vi.advanceTimersByTimeAsync(10); });
    expect(fetch).toHaveBeenCalledTimes(1);
    current = fixture({ status: "completed" });
    await act(async () => { await vi.advanceTimersByTimeAsync(WORKFLOW_POLL_MS + 10); });
    expect(fetch).toHaveBeenCalledTimes(2);
    await act(async () => { await vi.advanceTimersByTimeAsync(WORKFLOW_POLL_MS * 5); });
    expect(fetch).toHaveBeenCalledTimes(2);
  });
  it("refetches an idle list on reconnect to discover work started during disconnection", async () => {
    let current = fixture({ status: "completed" });
    mockApi(() => ({ definitions: [], runs: [current] }));
    render(<QueryClientProvider client={client()}><ListProbe /></QueryClientProvider>);
    await screen.findByText("completed");
    current = fixture({ id: NEXT });
    act(() => onlineManager.setOnline(false));
    act(() => onlineManager.setOnline(true));
    expect(await screen.findByText("running")).toBeTruthy();
  });
});
