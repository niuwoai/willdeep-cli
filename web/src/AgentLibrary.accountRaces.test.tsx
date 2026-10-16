// @vitest-environment jsdom
import { afterEach, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { AgentLibrary } from "./AgentLibrary";
import { agentText } from "./agentLibraryMessages";

afterEach(() => {cleanup();vi.unstubAllGlobals();});
const fixture={package:{definition:{schema:"willdeep.agent-definition.v1",definition_id:"user/audit-fixture",version:"1.0.0",name:{en:"Audit fixture","zh-Hans":"审计夹具"},purpose:"Synthetic fixture",base_role:"reviewer",instructions:"Synthetic instructions",input_modalities:["text"],tools:["workspace.read"],skills:[],forked_from:null},sha256:"a".repeat(64),signature:null},scope:"local",compatible:true,incompatible_reason:null,optional_skills_missing:[],installation:{origin:"local",enabled:true,pinned:false,dirty:false,binding:{provider_profile:null,model:null}}};
const a={origin:"https://fixture.invalid",user_id:"a",tenant_id:"synthetic"};
const b={...a,user_id:"b"};
const client=()=>new QueryClient({defaultOptions:{queries:{retry:false},mutations:{retry:false}}});

it("does not submit an A-selected file whose text finishes after switching to B",async()=>{
  const calls:string[]=[];
  vi.stubGlobal("fetch",vi.fn(async(url:string)=>{calls.push(url);return {ok:true,json:async()=>({items:[fixture],account:a})};}));
  const q=client();
  const view=render(<QueryClientProvider client={q}><AgentLibrary language="zh-CN" onTrial={vi.fn()}/></QueryClientProvider>);
  await screen.findByText("https://fixture.invalid · a / synthetic");
  let finish:(text:string)=>void=()=>undefined;
  const text=new Promise<string>(resolve=>{finish=resolve;});
  const input=view.container.querySelector('input[type="file"]')!;
  fireEvent.change(input,{target:{files:[{name:"synthetic-private-a.json",text:()=>text}]}});
  q.setQueryData(["agent-library"],{items:[fixture],account:b});
  await screen.findByText("https://fixture.invalid · b / synthetic");
  finish('{"private_account_a_synthetic_data":true}');
  await new Promise(resolve=>setTimeout(resolve,30));
  expect(calls.filter(url=>url.endsWith("/import"))).toHaveLength(0);
});

it("does not display a late A-export failure in the B account result",async()=>{
  let finish:(response:unknown)=>void=()=>undefined;
  vi.stubGlobal("fetch",vi.fn(async(url:string)=>url.endsWith("export")?new Promise(resolve=>{finish=resolve;}):{ok:true,json:async()=>({items:[fixture],account:a})}));
  const q=client();
  render(<QueryClientProvider client={q}><AgentLibrary language="zh-CN" onTrial={vi.fn()}/></QueryClientProvider>);
  await screen.findByText("https://fixture.invalid · a / synthetic");
  fireEvent.click(screen.getByText("更多操作"));
  fireEvent.click(screen.getByRole("button",{name:"导出"}));
  await waitFor(()=>expect(fetch).toHaveBeenCalledTimes(2));
  q.setQueryData(["agent-library"],{items:[fixture],account:b});
  await screen.findByText("https://fixture.invalid · b / synthetic");
  finish({ok:false,json:async()=>({error:"synthetic old account error"})});
  await new Promise(resolve=>setTimeout(resolve,30));
  expect(screen.queryByText(agentText("failed","zh-CN"))).toBeNull();
});

it("does not leave an unsaved definition through trial when discard is rejected",async()=>{
  vi.stubGlobal("fetch",vi.fn(async()=>({ok:true,json:async()=>({items:[fixture],account:a})})));
  vi.stubGlobal("confirm",vi.fn(()=>false));
  vi.stubGlobal("prompt",vi.fn(()=>"Synthetic trial task"));
  const onTrial=vi.fn();
  render(<QueryClientProvider client={client()}><AgentLibrary language="zh-CN" onTrial={onTrial}/></QueryClientProvider>);
  await screen.findByText("https://fixture.invalid · a / synthetic");
  fireEvent.click(screen.getByRole("button",{name:"创建"}));
  fireEvent.click(screen.getByText("更多操作"));
  fireEvent.click(screen.getByRole("button",{name:"试运行"}));
  expect(onTrial).not.toHaveBeenCalled();
  expect(window.confirm).toHaveBeenCalled();
});
