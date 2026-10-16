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
  fireEvent.click(screen.getByLabelText("智能体库操作"));
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
  fireEvent.click(screen.getByLabelText("智能体库操作"));
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

function installedFixture(id: string, origin = "local", revision = "a".repeat(64)) {
  return {package:{definition:{schema:"willdeep.agent-definition.v1",definition_id:id,version:"1.0.0",name:{en:id,"zh-Hans":id},purpose:"Fixture purpose",base_role:"reviewer",instructions:`Instructions ${id}`,input_modalities:["text"],tools:["workspace.read"],skills:[],forked_from:null},sha256:revision,signature:origin === "official" ? {key_id:"fixture",value:"fixture"} : null},scope:"local",compatible:true,incompatible_reason:null,optional_skills_missing:[],installation:{origin,enabled:true,pinned:false,dirty:false,binding:{provider_profile:null,model:null}}};
}
it("searches and filters a compact list while retaining advanced actions",async()=>{
  vi.stubGlobal("fetch",vi.fn(async()=>({ok:true,json:async()=>({items:[installedFixture("user/local"),installedFixture("official/review","official","b".repeat(64))],account:null})})));
  render(<QueryClientProvider client={new QueryClient({defaultOptions:{queries:{retry:false}}})}><AgentLibrary language="zh-CN" onTrial={vi.fn()}/></QueryClientProvider>);
  await screen.findByRole("button",{name:/user\/local/});
  fireEvent.change(screen.getByRole("textbox",{name:"搜索智能体"}),{target:{value:"official"}});
  expect(screen.queryByRole("button",{name:/user\/local/})).toBeNull();
  expect(screen.getByText("官方版本请先创建副本，再起草实验候选。")).toBeTruthy();
  expect(screen.queryByRole("button",{name:"智能体实验"})).toBeNull();
  expect(screen.getByRole("button",{name:"创建副本"})).toBeTruthy();
  fireEvent.change(screen.getByRole("textbox",{name:"搜索智能体"}),{target:{value:""}});
  fireEvent.change(screen.getByRole("combobox",{name:"来源"}),{target:{value:"local"}});
  fireEvent.click(screen.getByText("更多操作"));
  for (const name of ["编辑","创建副本","导出","本地模型绑定","固定版本","停用","加入账号同步","版本历史","试运行","删除"]) expect(screen.getByRole("button",{name})).toBeTruthy();
});

it("guards a dirty baseline switch and clears account-scoped candidate state",async()=>{
  let account={origin:"https://fixture.invalid",user_id:"a",tenant_id:"t"};
  const items=[installedFixture("user/first"),installedFixture("user/second","local","b".repeat(64))];
  const client=new QueryClient({defaultOptions:{queries:{retry:false}}});
  vi.stubGlobal("fetch",vi.fn(async(url:string)=>({ok:true,json:async()=>url.endsWith("rsi.list") ? {items:[]} : {items,account}})));
  vi.stubGlobal("confirm",vi.fn(()=>false));
  render(<QueryClientProvider client={client}><AgentLibrary language="zh-CN" mode="experiments" onTrial={vi.fn()}/></QueryClientProvider>);
  await screen.findByRole("option",{name:"user/first · 1.0.0"});
  fireEvent.change(screen.getByRole("combobox",{name:"选择一个已安装智能体"}),{target:{value:items[0].package.sha256}});
  fireEvent.change(screen.getByRole("textbox",{name:"候选专业指令"}),{target:{value:"Unsent private instructions"}});
  fireEvent.change(screen.getByRole("combobox",{name:"选择一个已安装智能体"}),{target:{value:items[1].package.sha256}});
  expect(window.confirm).toHaveBeenCalled();
  expect((screen.getByRole("textbox",{name:"候选专业指令"}) as HTMLTextAreaElement).value).toBe("Unsent private instructions");
  account={...account,user_id:"b"}; client.setQueryData(["agent-library"],{items,account});
  await screen.findByText("从智能体库选择用户定义作为基线。官方定义请先创建副本。");
  expect(screen.queryByRole("textbox",{name:"候选专业指令"})).toBeNull();
});

it("does not display late history from a previous account",async()=>{
  const item=installedFixture("user/first");
  let account={origin:"https://fixture.invalid",user_id:"a",tenant_id:"t"};
  let finish: (value:unknown)=>void=()=>undefined;
  vi.stubGlobal("fetch",vi.fn(async(url:string)=>url.endsWith("history") ? new Promise(resolve=>{finish=resolve;}) : {ok:true,json:async()=>({items:[item],account})}));
  const client=new QueryClient({defaultOptions:{queries:{retry:false},mutations:{retry:false}}});
  render(<QueryClientProvider client={client}><AgentLibrary language="zh-CN" onTrial={vi.fn()}/></QueryClientProvider>);
  await screen.findByRole("button",{name:/user\/first/});
  fireEvent.click(screen.getByText("更多操作")); fireEvent.click(screen.getByRole("button",{name:"版本历史"}));
  await waitFor(()=>expect(fetch).toHaveBeenCalledTimes(2));
  account={...account,user_id:"b"}; client.setQueryData(["agent-library"],{items:[item],account});
  await screen.findByText("https://fixture.invalid · b / t");
  finish({ok:true,json:async()=>[{...item.package,definition:{...item.package.definition,definition_id:"private-account-a-history"}}]});
  await new Promise(resolve=>setTimeout(resolve,0));
  expect(screen.queryByText(/private-account-a-history/)).toBeNull();
});
