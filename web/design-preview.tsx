// Isolated development-only fixture. Production index.html never imports this module.
import { useCallback, useRef, useState } from "react";
import { createRoot } from "react-dom/client";
import { ChakraProvider, defaultSystem } from "@chakra-ui/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { AgentLibrary } from "./src/AgentLibrary";
import { PluginRail, type RailSelection } from "./src/PluginRail";
import { agentText } from "./src/agentLibraryMessages";
import { messages, type Language } from "./src/i18n";
import { scheduleText } from "./src/scheduleMessages";
import { allowAgentNavigation } from "./src/agentNavigation";
import "./src/theme.css";
import "./src/styles.css";
import "./src/plugins.css";
import "./design-preview.css";

if (!import.meta.env.DEV) throw new Error("Design fixtures are development only");
const language: Language = new URLSearchParams(location.search).get("lang") === "en" ? "en" : new URLSearchParams(location.search).get("lang") === "ja" ? "ja" : "zh-CN";
const fixtureTheme = new URLSearchParams(location.search).get("theme");
document.documentElement.lang = language;
if (fixtureTheme === "light" || fixtureTheme === "dark") document.documentElement.dataset.theme = fixtureTheme;
else delete document.documentElement.dataset.theme;
const definition = {schema:"willdeep.agent-definition.v1",definition_id:"user/document-reviewer",version:"1.0.0",name:{en:"Document assistant","zh-Hans":"文档整理助手",ja:"文書整理アシスタント"},purpose:language === "zh-CN" ? "整理文档、提炼要点、生成结构化摘要。" : language === "ja" ? "文書を整理し、要点と構造化された概要を作成。" : "Organize documents, identify key points and create structured summaries.",base_role:"reviewer",instructions:language === "zh-CN" ? "1. 阅读并理解用户提供的文档内容。\n2. 按主题组织核心要点，生成清晰的结构化摘要。\n3. 尽量保留重要的引用信息。\n4. 对于不确定或缺少上下文的内容，给出合理的提示。\n5. 以简洁、专业的语言输出结果。" : language === "ja" ? "1. 提供された文書を読み、理解する。\n2. 主要な論点を整理する。\n3. 重要な引用を保持する。\n4. 不明な情報を明示する。\n5. 簡潔かつ専門的に回答する。" : "1. Read and understand the supplied document.\n2. Organize key points into a structured summary.\n3. Keep important references.\n4. Clearly identify missing context.\n5. Use concise, professional language.",input_modalities:["text"],tools:["workspace.read","repository.inspect"],skills:[],forked_from:null};
const packageValue = {definition,sha256:"a".repeat(64),signature:null};
const installed = {package:packageValue,scope:"local",compatible:true,incompatible_reason:null,optional_skills_missing:[],installation:{origin:"local",enabled:true,pinned:false,dirty:false,binding:{provider_profile:null,model:null}}};
const official = {...installed, package:{definition:{...definition,definition_id:"official/reviewer",name:{en:"Official reviewer","zh-Hans":"官方审查助手",ja:"公式レビュー"}},sha256:"b".repeat(64),signature:{key_id:"fixture-only",value:"not-a-real-signature"}},installation:{...installed.installation,origin:"official"}};
const experiments: Record<string, unknown>[] = [{experiment_id:"fixture-experiment-1",definition_id:definition.definition_id,baseline_revision:packageValue.sha256,candidate_revision:"c".repeat(64)}];
const calls: unknown[] = [];
// All operations are contained here. No request is forwarded to Runtime or some.im.
globalThis.fetch = async (input, init) => {
  const operation = String(input).split("/").at(-1);
  const params = JSON.parse(String(init?.body ?? "{}")) as Record<string, unknown>;
  calls.push({operation,params});
  let data: unknown;
  if (operation === "list") data = {items:[installed,official],account:{origin:"https://fixture.invalid",user_id:"design-fixture",tenant_id:"isolated"}};
  else if (operation === "rsi.list") data = {items:experiments,paid_execution_allowed:false};
  else if (operation === "rsi.prepare") {const experiment={experiment_id:params.id,definition_id:definition.definition_id,baseline_revision:packageValue.sha256,candidate_revision:"d".repeat(64)}; experiments.push(experiment); data={experiment,activated:false,paid_execution_allowed:false};}
  else if (operation === "rsi.candidate") data = {candidate:{candidate_id:params.id},activated:false};
  else if (operation === "rsi.run" || operation === "rsi.report") data = {attempts:6,evidence:Array.from({length:6},()=>({status:"passed"})),verdict:"offline_mock_only",activated:false};
  else if (operation === "rsi.upload") data = {uploaded_evidence:6,trust:"client_reported"};
  else if (operation === "history") data = [packageValue];
  else if (operation === "export" || operation === "preview") data=packageValue;
  else if (operation === "catalog") data={items:[official.package.definition]};
  else if (operation === "sync") data={conflicts:[],conflict_details:[]};
  else return new Response(JSON.stringify({error:"Isolated design fixture: unsupported action"}),{status:400,headers:{"Content-Type":"application/json"}});
  return new Response(JSON.stringify(data),{status:200,headers:{"Content-Type":"application/json"}});
};
export function Preview() {
  const dirty=useRef(false);
  const trackDirty=useCallback((value:boolean)=>{dirty.current=value;},[]);
  const [selection,setSelection] = useState<RailSelection>({kind:location.hash === "#agents" ? "agents" : "agent-experiments"});
  const navigate = (next: RailSelection) => {if (next.kind === selection.kind || !allowAgentNavigation(dirty.current,language)) return; setSelection(next); history.replaceState(null,"",next.kind === "agents" ? "#agents" : "#agent-experiments");};
  return <div className="design-fixture-shell"><PluginRail entries={[]} selection={selection} messages={messages[language]} brandLabel={agentText("brand",language)} schedulesLabel={scheduleText("title",language)} agentsLabel={agentText("title",language)} experimentsLabel={agentText("experimentTitle",language)} agentsGroupLabel={agentText("navigationGroup",language)} onSelect={navigate} /><AgentLibrary language={language} mode={selection.kind === "agents" ? "library" : "experiments"} onDirty={trackDirty} onNavigate={mode=>navigate({kind:mode === "library" ? "agents" : "agent-experiments"})} onTrial={()=>undefined} /></div>;
}
const queryClient = new QueryClient({defaultOptions:{queries:{retry:false},mutations:{retry:false}}});
createRoot(document.getElementById("root")!).render(<QueryClientProvider client={queryClient}><ChakraProvider value={defaultSystem}><Preview /></ChakraProvider></QueryClientProvider>);
Object.defineProperty(window,"designFixtureCalls",{get:()=>structuredClone(calls)});
