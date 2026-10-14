// @vitest-environment jsdom
import { afterEach, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { AgentLibrary } from "./AgentLibrary";
import { nextAgentVersion } from "./agentLibraryModel";

afterEach(() => {cleanup();vi.unstubAllGlobals();});
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
