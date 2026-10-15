// @vitest-environment jsdom
import { afterEach, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { AgentRsiPanel } from "./AgentRsiPanel";

afterEach(() => {cleanup();vi.unstubAllGlobals();});
it("registers a single instruction candidate without selecting or installing it", async () => {
  const calls: {operation: string; params: Record<string, unknown>}[] = [];
  vi.stubGlobal("fetch", vi.fn(async (url: string, init: RequestInit) => {
    const operation = url.split("/").at(-1)!;
    calls.push({operation, params: JSON.parse(String(init.body)) as Record<string, unknown>});
    return {ok: true, json: async () => operation === "rsi.list" ? {items: []} : {activated: false, experiment:{experiment_id:"mock-experiment"}}};
  }));
  const baseline = {definition: {definition_id: "user/reviewer", version: "1.0.0-rc1", instructions: "Review evidence.", tools: ["workspace.read"]}, sha256: "a".repeat(64), signature: null};
  render(<QueryClientProvider client={new QueryClient({defaultOptions: {queries: {retry: false}, mutations: {retry: false}}})}><AgentRsiPanel language="zh-CN" baseline={baseline} onClose={vi.fn()} /></QueryClientProvider>);
  fireEvent.change(screen.getByRole("textbox", {name: "候选专业指令"}), {target: {value: "Review evidence and minimal reproductions."}});
  fireEvent.click(screen.getByRole("button", {name: "登记离线配对实验"}));
  await waitFor(() => expect(calls.some(call => call.operation === "rsi.prepare")).toBe(true));
  const prepared = calls.find(call => call.operation === "rsi.prepare")!;
  expect(prepared.params).toMatchObject({baseline_package: baseline, candidate_definition: {version: "1.0.0-rc2", instructions: "Review evidence and minimal reproductions.", forked_from: baseline.sha256, tools: ["workspace.read"]}});
  expect(calls.some(call => ["install", "save", "select", "sync", "share"].includes(call.operation))).toBe(false);
  expect(screen.getByText(/当前仅允许免费离线验收/)).toBeTruthy();
  expect(screen.getByText("离线配对实验已登记。")).toBeTruthy();
  expect(screen.getByText("当前使用版本未改变，也未发布任何版本。")).toBeTruthy();
  expect(document.querySelector("details[open]")).toBeNull();
});

it("requires an official fork before allowing candidate requests", () => {
  const calls: string[] = [];
  vi.stubGlobal("fetch", vi.fn(async (url: string) => {calls.push(url); return {ok:true,json:async()=>({items:[]})};}));
  const baseline = {definition: {definition_id:"official/reviewer",version:"1.0.0",instructions:"Official instructions."},sha256:"a".repeat(64),signature:{key_id:"test-only",value:"test-only"}};
  render(<QueryClientProvider client={new QueryClient({defaultOptions:{queries:{retry:false}}})}><AgentRsiPanel language="zh-CN" baseline={baseline} onClose={vi.fn()} /></QueryClientProvider>);
  expect(screen.getByText("官方版本请先创建副本，再起草实验候选。")).toBeTruthy();
  expect((screen.getByRole("button",{name:"登记离线配对实验"}) as HTMLButtonElement).disabled).toBe(true);
  fireEvent.click(screen.getByRole("button",{name:"仅保存候选"}));
  expect(calls.every(url=>url.endsWith("rsi.list"))).toBe(true);
});

it("clears draft and result when the account/baseline identity changes", async () => {
  vi.stubGlobal("fetch",vi.fn(async()=>({ok:true,json:async()=>({items:[]})})));
  const baseline = {definition:{definition_id:"user/reviewer",version:"1.0.0",instructions:"First account instructions."},sha256:"a".repeat(64),signature:null};
  const client=new QueryClient({defaultOptions:{queries:{retry:false}}});
  const view=render(<QueryClientProvider client={client}><AgentRsiPanel key="account-a" accountScope="account-a" language="zh-CN" baseline={baseline} onClose={vi.fn()} /></QueryClientProvider>);
  fireEvent.change(screen.getByRole("textbox",{name:"候选专业指令"}),{target:{value:"Unsent private draft."}});
  const second={...baseline,definition:{...baseline.definition,instructions:"Second account instructions."},sha256:"b".repeat(64)};
  view.rerender(<QueryClientProvider client={client}><AgentRsiPanel key="account-b" accountScope="account-b" language="zh-CN" baseline={second} onClose={vi.fn()} /></QueryClientProvider>);
  expect((screen.getByRole("textbox",{name:"候选专业指令"}) as HTMLTextAreaElement).value).toBe("Second account instructions.");
  expect(screen.queryByText("Unsent private draft.")).toBeNull();
});
