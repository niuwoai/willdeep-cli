// @vitest-environment jsdom
import { afterEach, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { AgentLibrary } from "./AgentLibrary";
import { ForeignAgentImport } from "./ForeignAgentImport";
import { agentLibraryParams, agentRevisionConflict } from "./agentLibraryModel";
import { agentText } from "./agentLibraryMessages";

const scope = {library_id:"fixture-library",account:null};
const fixture = {
  scope:"local",installation_revision:7,definition_revision:"a".repeat(64),compatible:true,incompatible_reason:null,optional_skills_missing:[],
  installation:{installation_revision:7,origin:"local",enabled:true,pinned:false,dirty:false,binding:{provider_profile:null,model:null}},
  package:{definition:{schema:"willdeep.agent-definition.v1",definition_id:"user/reviewer",version:"1.0.0-rc9",name:{en:"Reviewer"},purpose:"Review changes",base_role:"reviewer",instructions:"Review the patch",input_modalities:["text"],tools:["workspace.read"],skills:[],forked_from:null},sha256:"a".repeat(64),signature:null},
};
afterEach(() => {cleanup();vi.unstubAllGlobals();});
function setup(fail = false) {
  const calls: {operation:string;params:Record<string,unknown>}[] = [];
  const client = new QueryClient({defaultOptions:{queries:{retry:false},mutations:{retry:false}}});
  vi.stubGlobal("fetch",vi.fn(async (url:string, init?:RequestInit) => {
    if (url.includes("model-routing")) return {ok:true,json:async()=>({providers:[]})};
    if (url.includes("/models?")) return {ok:true,json:async()=>({models:[]})};
    const operation = url.split("/").at(-1)!;
    const params = JSON.parse(String(init?.body ?? "{}")) as Record<string,unknown>;
    calls.push({operation,params});
    return {ok:!(fail && operation === "save"),json:async()=> operation === "list" ? {items:[fixture],account:null,scope} : operation === "catalog" ? {items:[]} : operation === "history" ? [fixture.package] : fail ? {error:"agent_revision_conflict: synthetic stale revision"} : {saved:true}};
  }));
  render(<QueryClientProvider client={client}><AgentLibrary language="en" onTrial={vi.fn()} /></QueryClientProvider>);
  return {calls,client};
}
function precondition(params:Record<string,unknown>) {
  expect(params.expected_revision).toBe(7);
  expect(params.expected_scope).toEqual(scope);
  expect(typeof params.request_id).toBe("string");
  expect(params.request_id).not.toBe("");
}
it("submits editing against the selected generation and preserves drafts after conflicts",async()=>{
  const {calls,client} = setup(true);
  await screen.findByRole("button",{name:/Reviewer/});
  fireEvent.click(screen.getByText("More actions")); fireEvent.click(screen.getByRole("button",{name:"Edit"}));
  const editor = screen.getByRole("textbox",{name:"Definition JSON (tools and embedded skills)"}) as HTMLTextAreaElement;
  const draft = editor.value;
  expect(JSON.parse(draft).version).toBe("1.0.0-rc10");
  client.setQueryData(["agent-library"],{items:[{...fixture,installation_revision:8,installation:{...fixture.installation,installation_revision:8}}],account:null,scope});
  fireEvent.click(screen.getByRole("button",{name:"Validate and save"}));
  await screen.findByText(agentText("revisionConflict","en"));
  const saved = calls.find(call=>call.operation === "save")!;
  precondition(saved.params);
  expect(editor.value).toBe(draft);
});
it.each([["Disable","enable"],["Pin","pin"],["Fork","fork"]])("passes local CAS and scope for %s",async(label,operation)=>{
  vi.stubGlobal("prompt",vi.fn(()=>"user/new-reviewer"));
  const {calls} = setup();
  await screen.findByRole("button",{name:/Reviewer/});
  fireEvent.click(screen.getByText("More actions")); fireEvent.click(screen.getByRole("button",{name:label}));
  await waitFor(()=>expect(calls.some(call=>call.operation === operation)).toBe(true));
  precondition(calls.find(call=>call.operation === operation)!.params);
});
it("binds and rolls back with the installation generation rather than the body hash",async()=>{
  const {calls} = setup();
  await screen.findByRole("button",{name:/Reviewer/});
  fireEvent.click(screen.getByText("More actions")); fireEvent.click(screen.getByRole("button",{name:"Local model binding"}));
  fireEvent.click(await screen.findByRole("button",{name:"Validate and save"}));
  await waitFor(()=>expect(calls.some(call=>call.operation === "bind")).toBe(true));
  precondition(calls.find(call=>call.operation === "bind")!.params);
  await waitFor(()=>expect(screen.queryByRole("button",{name:"Validate and save"})).toBeNull());
  fireEvent.click(screen.getByRole("button",{name:"History"}));
  fireEvent.click(await screen.findByRole("button",{name:"Select and pin"}));
  await waitFor(()=>expect(calls.some(call=>call.operation === "rollback")).toBe(true));
  const rollback = calls.find(call=>call.operation === "rollback")!.params;
  precondition(rollback); expect(rollback.revision).toBe(fixture.package.sha256);
});
it("imports a newer JSON package with the installed CAS snapshot",async()=>{
  const {calls} = setup();
  await screen.findByRole("button",{name:/Reviewer/});
  const input = document.querySelector('input[type="file"][accept="application/json,.json"]')!;
  fireEvent.change(input,{target:{files:[{text:async()=>JSON.stringify({...fixture.package,definition:{...fixture.package.definition,version:"1.0.0-rc10"}})}]}});
  await waitFor(()=>expect(calls.some(call=>call.operation === "import")).toBe(true));
  precondition(calls.find(call=>call.operation === "import")!.params);
});
it("foreign import captures scope with the preview and reports a localized conflict",async()=>{
  const calls:Record<string,unknown>[] = [];
  vi.stubGlobal("fetch",vi.fn(async(url:string,init?:RequestInit)=>{
    if (url.endsWith("/import")) {calls.push(JSON.parse(String(init?.body)));return {ok:false,json:async()=>({error:"agent_scope_conflict: fixture"})};}
    return {ok:true,json:async()=>url.includes("model-routing") ? {providers:[]} : {models:[]}};
  }));
  const client = new QueryClient({defaultOptions:{queries:{retry:false}}});
  const view=render(<QueryClientProvider client={client}><ForeignAgentImport language="en" scope={scope} onDone={vi.fn()}/></QueryClientProvider>);
  fireEvent.click(screen.getByText("Import Hermes / Multica"));
  fireEvent.change(screen.getByRole("combobox",{name:"Source format"}),{target:{value:"multica"}});
  fireEvent.change(screen.getByLabelText("Agent JSON file"),{target:{files:[{size:100,text:async()=>JSON.stringify({name:"Imported",instructions:"Review imported changes"})}]}});
  await waitFor(()=>expect((screen.getByLabelText("Agent JSON file") as HTMLInputElement).files).toHaveLength(1));
  await new Promise(resolve=>setTimeout(resolve,0));
  fireEvent.click(screen.getByRole("button",{name:"Preview and compare"}));
  await screen.findByRole("button",{name:"Validate and save"});
  view.rerender(<QueryClientProvider client={client}><ForeignAgentImport language="en" scope={{...scope,library_id:"other"}} onDone={vi.fn()}/></QueryClientProvider>);
  fireEvent.click(screen.getByRole("button",{name:"Validate and save"}));
  await screen.findByText(agentText("revisionConflict","en"));
  expect(calls[0].expected_scope).toEqual(scope);
  expect(calls[0].expected_revision).toBeUndefined();
  expect(typeof calls[0].request_id).toBe("string");
});
it("adds request preconditions only to local management writes and covers all conflict messages",()=>{
  const read = {id:"user/reviewer"};
  expect(agentLibraryParams("get",read,scope)).toBe(read);
  expect(agentLibraryParams("bind",{...read,expected_scope:"untrusted"},scope).expected_scope).toEqual(scope);
  for (const code of ["agent_revision_conflict","agent_revision_required","agent_scope_conflict","agent_request_conflict"]) expect(agentRevisionConflict(new Error(code))).toBe(true);
  for (const language of ["zh-CN","en","ja"] as const) expect(agentText("revisionConflict",language)).not.toBe("revisionConflict");
});
