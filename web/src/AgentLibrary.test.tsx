// @vitest-environment jsdom
import { afterEach, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { AgentLibrary } from "./AgentLibrary";
import { nextAgentVersion } from "./agentLibraryModel";

afterEach(() => {cleanup();vi.unstubAllGlobals();});
it.each([["保留本机并上传", "keep_local"], ["采用云端状态", "use_cloud"]])("offers an explicit conflict choice: %s", async (label, choice) => {
  const resolveCalls: unknown[] = [];
  const conflict = {definition_id:"user/reviewer",expected_local_revision:"a".repeat(64),expected_remote_etag:"cloud-v2",remote_revision:"b".repeat(64),remote_deleted:true,local_version:"1.0.0-rc2",remote_version:"1.0.0-rc2",pinned:true};
  vi.stubGlobal("fetch",vi.fn(async (url:string, init:RequestInit) => {
    const operation = url.split("/").at(-1)!;
    if(operation === "resolve") resolveCalls.push(JSON.parse(String(init.body)));
    return {ok:true,json:async()=>operation === "list" ? {items:[],account:{origin:"https://some.im",user_id:"test-a",tenant_id:"test-tenant"}} : operation === "sync" ? {conflicts:[conflict.definition_id],conflict_details:[conflict]} : {resolved:conflict.definition_id,version:"1.0.0-rc9",deleted:choice === "use_cloud"}};
  }));
  render(<QueryClientProvider client={new QueryClient({defaultOptions:{queries:{retry:false},mutations:{retry:false}}})}><AgentLibrary language="zh-CN" onTrial={vi.fn()} /></QueryClientProvider>);
  await screen.findByText("https://some.im · test-a / test-tenant");
  fireEvent.click(screen.getByRole("button",{name:"主动同步"}));
  const region=await screen.findByRole("region",{name:"待解决的同步冲突"});
  expect(region.textContent).toContain("云端已删除");
  expect(region.textContent).toContain("本机已固定版本");
  if(choice === "keep_local") {
    const version=screen.getByRole("textbox",{name:"保留本机时的新版本（可选）"});
    expect((version as HTMLInputElement).value).toBe("1.0.0-rc3");
    fireEvent.change(version,{target:{value:""}});
    expect((screen.getByRole("button",{name:label}) as HTMLButtonElement).disabled).toBe(true);
    fireEvent.change(version,{target:{value:"1.0.0-rc9"}});
  }
  fireEvent.click(screen.getByRole("button",{name:label}));
  await waitFor(()=>expect(resolveCalls).toEqual([{id:conflict.definition_id,choice,expected_local_revision:conflict.expected_local_revision,expected_remote_etag:conflict.expected_remote_etag,...(choice === "keep_local" ? {new_version:"1.0.0-rc9"} : {}),expected_account:{origin:"https://some.im",user_id:"test-a",tenant_id:"test-tenant"}}]));
  await waitFor(()=>expect(screen.queryByRole("region",{name:"待解决的同步冲突"})).toBeNull());
  await screen.findByText(choice === "keep_local" ? "冲突已解决，当前版本：1.0.0-rc9。历史版本仍保留。" : "已接受云端删除，本机历史版本仍保留。");
});
it("removes conflict choices when the displayed account changes",async()=>{
  const client=new QueryClient({defaultOptions:{queries:{retry:false},mutations:{retry:false}}});
  vi.stubGlobal("fetch",vi.fn(async(url:string)=>({ok:true,json:async()=>url.endsWith("list") ? {items:[],account:{origin:"https://some.im",user_id:"a",tenant_id:"t"}} : {conflicts:["user/reviewer"],conflict_details:[{definition_id:"user/reviewer",expected_local_revision:"a".repeat(64),expected_remote_etag:"etag",remote_revision:"b".repeat(64),remote_deleted:false,local_version:"1.0.0",remote_version:"1.0.0",pinned:false}]}})));
  render(<QueryClientProvider client={client}><AgentLibrary language="zh-CN" onTrial={vi.fn()}/></QueryClientProvider>);
  await screen.findByText("https://some.im · a / t");
  fireEvent.click(screen.getByRole("button",{name:"主动同步"}));
  await screen.findByRole("region",{name:"待解决的同步冲突"});
  client.setQueryData(["agent-library"],{items:[],account:{origin:"https://some.im",user_id:"b",tenant_id:"t"}});
  await screen.findByText("https://some.im · b / t");
  expect(screen.queryByRole("region",{name:"待解决的同步冲突"})).toBeNull();
});
it("editing advances only the prerelease and preserves formal version", () => {
  expect(nextAgentVersion("1.2.3")).toBe("1.2.3-rc1");
  expect(nextAgentVersion("1.2.3-rc4")).toBe("1.2.3-rc5");
});
it("creates and validates a complete portable definition through the Runtime API", async () => {
  const calls: {operation: string;params: Record<string,unknown>}[] = [];
  vi.stubGlobal("fetch", vi.fn(async (url: string, init: RequestInit) => {
    const operation = url.split("/").at(-1)!;
    const params = JSON.parse(String(init.body)) as Record<string,unknown>;
    calls.push({operation,params});
    return {ok:true,json:async () => operation === "list" ? {items:[],account:null} : {saved:true}};
  }));
  render(<QueryClientProvider client={new QueryClient({defaultOptions:{queries:{retry:false},mutations:{retry:false}}})}><AgentLibrary language="zh-CN" onTrial={vi.fn()} /></QueryClientProvider>);
  await screen.findByText("服务账号尚未登录。请在终端运行 willdeep agents login；模型 Provider 可独立配置。");
  fireEvent.click(screen.getByRole("button",{name:"创建"}));
  const editor = screen.getByRole("textbox",{name:"完整定义 JSON（可配置工具和内嵌技能）"});
  const definition = JSON.parse((editor as HTMLTextAreaElement).value) as Record<string,unknown>;
  definition.definition_id = "user/team-reviewer";
  fireEvent.change(editor,{target:{value:JSON.stringify(definition)}});
  fireEvent.click(screen.getByRole("button",{name:"校验并保存"}));
  await waitFor(() => expect(calls.some(call => call.operation === "save")).toBe(true));
  const save = calls.find(call => call.operation === "save")!;
  expect(JSON.parse(String(save.params.definition_text))).toMatchObject({schema:"willdeep.agent-definition.v1",definition_id:"user/team-reviewer",forked_from:null});
  expect(calls.filter(call => call.operation === "list").length).toBeGreaterThan(1);
});
