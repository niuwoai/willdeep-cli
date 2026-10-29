// @vitest-environment jsdom
import { afterEach, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { AgentModelBinding } from "./AgentModelBinding";
afterEach(()=>{cleanup();vi.unstubAllGlobals();});
it("discovers models for the selected Provider and keeps a previously bound model", async()=>{
  const onSave=vi.fn();
  vi.stubGlobal("fetch",vi.fn(async(url:string)=>({ok:true,json:async()=>url.includes("model-routing") ? {providers:[{id:"provider-a",provider:"openai-compatible",model:"model-a"},{id:"provider-b",provider:"anthropic",model:"model-b"}]} : {models:url.includes("provider-b") ? ["model-b"] : ["model-a"],warning:null}})));
  render(<QueryClientProvider client={new QueryClient({defaultOptions:{queries:{retry:false}}})}><AgentModelBinding language="zh-CN" binding={{provider_profile:"provider-a",model:"old-model"}} pending={false} onSave={onSave} onCancel={vi.fn()} /></QueryClientProvider>);
  expect(screen.getByRole("option",{name:"old-model"})).toBeTruthy();
  await screen.findByRole("option",{name:"provider-b"});
  fireEvent.change(screen.getByRole("combobox",{name:"模型 Provider"}),{target:{value:"provider-b"}});
  await screen.findByRole("option",{name:"model-b"});
  fireEvent.change(screen.getByRole("combobox",{name:"模型"}),{target:{value:"model-b"}});
  fireEvent.click(screen.getByRole("button",{name:"校验并保存"}));
  expect(onSave).toHaveBeenCalledWith({provider_profile:"provider-b",model:"model-b",context_window:null,max_turns:null});
  expect(fetch).toHaveBeenCalledWith("/api/schedules/models?profile=provider-b");
});
it("edits execution limits and prevents invalid budgets", () => {
  const onSave = vi.fn();
  vi.stubGlobal("fetch", vi.fn(async () => ({ok:true,json:async()=>({providers:[],models:[]})})));
  render(<QueryClientProvider client={new QueryClient()}><AgentModelBinding language="zh-CN" binding={{provider_profile:null,model:null,context_window:32768,max_turns:8}} pending={false} onSave={onSave} onCancel={vi.fn()} /></QueryClientProvider>);
  const turns = screen.getByRole("spinbutton", {name:"最大轮次"});
  fireEvent.change(turns, {target:{value:"0"}});
  fireEvent.click(screen.getByRole("button", {name:"校验并保存"}));
  expect(onSave).not.toHaveBeenCalled();
  fireEvent.change(turns, {target:{value:"12"}});
  fireEvent.click(screen.getByRole("button", {name:"校验并保存"}));
  expect(onSave).toHaveBeenCalledWith({provider_profile:null,model:null,context_window:32768,max_turns:12});
});
