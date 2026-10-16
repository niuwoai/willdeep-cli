// @vitest-environment jsdom
import { afterEach, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { AgentRsiPanel } from "./AgentRsiPanel";

afterEach(() => {cleanup();localStorage.clear();vi.unstubAllGlobals();});
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
  fireEvent.click(screen.getByRole("button", {name: "准备免费验证"}));
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
  expect((screen.getByRole("button",{name:"准备免费验证"}) as HTMLButtonElement).disabled).toBe(true);
  fireEvent.click(screen.getByRole("button",{name:"保存候选"}));
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

it("isolates persistent presentation notes and late results by account even for the same revision", async () => {
  let finish: (value: unknown) => void = () => undefined;
  const calls: Record<string, unknown>[] = [];
  vi.stubGlobal("fetch",vi.fn(async(url:string,init:RequestInit) => {
    calls.push(JSON.parse(String(init.body)) as Record<string,unknown>);
    if(url.endsWith("rsi.prepare")) return new Promise(resolve => {finish=resolve;});
    return {ok:true,json:async()=>({items:[]})};
  }));
  const baseline={definition:{definition_id:"user/review",version:"1.0.0",instructions:"Original"},sha256:"a".repeat(64),signature:null};
  const client=new QueryClient({defaultOptions:{queries:{retry:false},mutations:{retry:false}}});
  const view=render(<QueryClientProvider client={client}><AgentRsiPanel accountScope="a" language="zh-CN" baseline={baseline} onClose={vi.fn()} /></QueryClientProvider>);
  fireEvent.change(screen.getByRole("textbox",{name:"实验备注（仅本机）"}),{target:{value:"Private account A note"}});
  fireEvent.change(screen.getByRole("textbox",{name:"候选专业指令"}),{target:{value:"Modified instructions"}});
  fireEvent.click(screen.getByRole("button",{name:"准备免费验证"}));
  await waitFor(()=>expect(calls.length).toBeGreaterThan(1));
  view.rerender(<QueryClientProvider client={client}><AgentRsiPanel accountScope="b" language="zh-CN" baseline={baseline} onClose={vi.fn()} /></QueryClientProvider>);
  expect((screen.getByRole("textbox",{name:"实验备注（仅本机）"}) as HTMLInputElement).value).toBe("");
  expect((screen.getByRole("textbox",{name:"候选专业指令"}) as HTMLTextAreaElement).value).toBe("Original");
  finish({ok:true,json:async()=>({experiment:{experiment_id:"private-a"}})});
  await new Promise(resolve=>setTimeout(resolve,0));
  expect(screen.queryByText("离线配对实验已登记。")).toBeNull();
  expect(JSON.stringify(calls)).not.toContain("Private account A note");
  view.rerender(<QueryClientProvider client={client}><AgentRsiPanel accountScope="a" language="zh-CN" baseline={baseline} onClose={vi.fn()} /></QueryClientProvider>);
  expect((screen.getByRole("textbox",{name:"实验备注（仅本机）"}) as HTMLInputElement).value).toBe("Private account A note");
});

it("clears the dirty guard after successful candidate save and leaves the baseline read-only",async()=>{
  vi.stubGlobal("fetch",vi.fn(async(url:string)=>({ok:true,json:async()=>url.endsWith("rsi.list") ? {items:[]} : {candidate:{candidate_id:"saved"}}})));
  const dirty=vi.fn();
  const baseline={definition:{definition_id:"user/review",version:"1.0.0",instructions:"Original"},sha256:"a".repeat(64),signature:null};
  render(<QueryClientProvider client={new QueryClient({defaultOptions:{queries:{retry:false}}})}><AgentRsiPanel accountScope="a" language="zh-CN" baseline={baseline} onClose={vi.fn()} onDirty={dirty}/></QueryClientProvider>);
  fireEvent.change(screen.getByRole("textbox",{name:"候选专业指令"}),{target:{value:"New instructions"}});
  await waitFor(()=>expect(dirty).toHaveBeenLastCalledWith(true));
  expect(screen.getByText("Original")).toBeTruthy();
  fireEvent.click(screen.getByRole("button",{name:"保存候选"}));
  await screen.findByText("候选已独立保存。");
  await waitFor(()=>expect(dirty).toHaveBeenLastCalledWith(false));
});

it("keeps report and grant-upload explicit with no implicit model or activation actions",async()=>{
  const calls: {operation:string;params:unknown}[]=[];
  vi.stubGlobal("prompt",vi.fn(()=>"approved-grant"));
  vi.stubGlobal("fetch",vi.fn(async(url:string,init:RequestInit)=>{
    const operation=url.split("/").at(-1)!; calls.push({operation,params:JSON.parse(String(init.body))});
    return {ok:true,json:async()=>operation === "rsi.list" ? {items:[{experiment_id:"saved",definition_id:"user/review",baseline_revision:"a",candidate_revision:"b"}]} : operation === "rsi.upload" ? {uploaded_evidence:2} : {attempts:6,evidence:[],verdict:"inconclusive"}};
  }));
  render(<QueryClientProvider client={new QueryClient({defaultOptions:{queries:{retry:false}}})}><AgentRsiPanel language="zh-CN" baseline={null} onClose={vi.fn()}/></QueryClientProvider>);
  await screen.findByText(/user\/review · saved/);
  fireEvent.click(screen.getByRole("button",{name:"运行免费验证"}));
  await screen.findByText(/离线验收尚未完成/);
  fireEvent.click(screen.getByLabelText("更多操作"));
  fireEvent.click(screen.getByRole("button",{name:"授权上传摘要"}));
  await screen.findByText(/已上传 2 条/);
  expect(calls.find(call=>call.operation === "rsi.upload")?.params).toEqual({id:"saved",grant_id:"approved-grant"});
  expect(calls.every(call=>["rsi.list","rsi.run","rsi.upload"].includes(call.operation))).toBe(true);
});
