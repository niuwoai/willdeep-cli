// @vitest-environment jsdom
import { useCallback, useRef, useState } from "react";
import { afterEach, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { ChakraProvider, defaultSystem } from "@chakra-ui/react";
import { AgentLibrary } from "./AgentLibrary";
import { PluginRail, type RailSelection } from "./PluginRail";
import { messages } from "./i18n";
import { agentText } from "./agentLibraryMessages";
import { allowAgentNavigation } from "./agentNavigation";
afterEach(()=>{cleanup();vi.unstubAllGlobals();});

// Production App and the isolated preview share this onDirty -> navigation gate wiring.
function Workspace() {
  const dirty=useRef(false);
  const track=useCallback((value:boolean)=>{dirty.current=value;},[]);
  const [selection,setSelection]=useState<RailSelection>({kind:"agent-experiments"});
  const navigate=(next:RailSelection)=>{if(next.kind !== selection.kind && allowAgentNavigation(dirty.current,"zh-CN")) setSelection(next);};
  return <><PluginRail entries={[]} selection={selection} messages={messages["zh-CN"]} schedulesLabel="Fixture" agentsLabel={agentText("title","zh-CN")} experimentsLabel={agentText("experimentTitle","zh-CN")} onSelect={navigate}/><AgentLibrary language="zh-CN" mode={selection.kind === "agents" ? "library" : "experiments"} onDirty={track} onTrial={()=>undefined} onNavigate={mode=>setSelection({kind:mode === "library" ? "agents" : "agent-experiments"})}/></>;
}
it("connects candidate dirty state to the real rail gate, then clears it after save",async()=>{
  const item={package:{definition:{definition_id:"user/review",version:"1.0.0",name:{en:"Reviewer"},instructions:"Original",purpose:"Review",base_role:"reviewer"},sha256:"a".repeat(64),signature:null},scope:"local",compatible:true,optional_skills_missing:[],installation:{origin:"local",enabled:true,pinned:false,dirty:false,binding:{provider_profile:null,model:null}}};
  vi.stubGlobal("fetch",vi.fn(async(url:string)=>({ok:true,json:async()=>url.endsWith("/list") ? {items:[item],account:null} : url.endsWith("rsi.list") ? {items:[]} : {candidate:{candidate_id:"saved"}}})));
  vi.stubGlobal("confirm",vi.fn(()=>false));
  render(<QueryClientProvider client={new QueryClient({defaultOptions:{queries:{retry:false}}})}><ChakraProvider value={defaultSystem}><Workspace/></ChakraProvider></QueryClientProvider>);
  await screen.findByRole("option",{name:"Reviewer · 1.0.0"});
  fireEvent.change(screen.getByRole("combobox",{name:"选择一个已安装智能体"}),{target:{value:item.package.sha256}});
  fireEvent.change(screen.getByRole("textbox",{name:"候选专业指令"}),{target:{value:"Unsaved change"}});
  fireEvent.click(screen.getByRole("button",{name:"智能体库"}));
  expect(window.confirm).toHaveBeenCalledOnce();
  expect(screen.getByRole("textbox",{name:"候选专业指令"})).toBeTruthy();
  fireEvent.click(screen.getByRole("button",{name:"保存候选"}));
  await screen.findByText("候选已独立保存。");
  await waitFor(()=>expect((screen.getByRole("button",{name:"保存候选"}) as HTMLButtonElement).disabled).toBe(false));
  fireEvent.click(screen.getByRole("button",{name:"智能体库"}));
  await screen.findByRole("heading",{name:"智能体库"});
  expect(window.confirm).toHaveBeenCalledOnce();
});
