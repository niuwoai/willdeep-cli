// @vitest-environment jsdom
import { afterEach, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { ChakraProvider, defaultSystem } from "@chakra-ui/react";
import { ModelPicker } from "./ModelPicker";
import { modelText } from "./modelMessages";

afterEach(() => { cleanup(); vi.unstubAllGlobals(); });

const t = modelText("zh-CN");

function mount(over: {
  current?: string | null;
  sessionId?: string | null;
  default?: { model: { provider_id: string; provider: string; model: string } | null; writable: boolean };
  post?: (url: string, init?: RequestInit) => Response;
} = {}) {
  const picked: { model: string; providerId: string }[] = [];
  const applied: string[] = [];
  const opened: boolean[] = [];
  const posts: { url: string; body: unknown }[] = [];
  vi.stubGlobal("fetch", vi.fn(async (url: string, init?: RequestInit) => {
    if (init?.method === "POST") {
      posts.push({ url, body: JSON.parse(String(init.body)) });
      return over.post ? over.post(url, init) : Response.json({});
    }
    if (url === "/api/models/default") {
      return Response.json(over.default ?? { model: null, writable: true });
    }
    if (url === "/api/settings/model-routing") {
      return Response.json({ default_provider: "main", providers: [
        { id: "main", provider: "openai-compatible", model: "gpt-5.1" },
        { id: "backup", provider: "backup-gateway", model: "claude-sonnet-5-5" },
      ] });
    }
    const profile = new URL(url, "http://localhost").searchParams.get("profile");
    return Response.json({ models: profile === "backup" ? ["claude-sonnet-5-5"] : ["gpt-5.1", "gpt-5.1-mini"], warning: null });
  }));
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const view = render(<QueryClientProvider client={client}>
    <ChakraProvider value={defaultSystem}>
      <ModelPicker
        sessionId={over.sessionId === undefined ? "s-1" : over.sessionId} language="zh-CN" current={over.current ?? null} open
        onOpenChange={(open) => { opened.push(open); }}
        onPicked={(model, providerId) => { picked.push({ model, providerId }); }}
        onApplied={(text) => { applied.push(text); }}
      />
    </ChakraProvider>
  </QueryClientProvider>);
  return { ...view, picked, applied, opened, posts };
}

it("selects a provider first, searches only its models, and marks the current model", async () => {
  mount({ current: "gpt-5.1" });
  const provider = await screen.findByRole("combobox", { name: t.provider });
  const search = screen.getByRole("searchbox", { name: t.searchModels });
  await screen.findByRole("button", { name: "gpt-5.1" });
  expect((provider as HTMLSelectElement).value).toBe("main");
  expect(screen.queryByRole("button", { name: "claude-sonnet-5-5" })).toBeNull();

  fireEvent.change(search, { target: { value: "mini" } });
  expect(screen.getByRole("button", { name: "gpt-5.1-mini" })).toBeTruthy();
  expect(screen.queryByRole("button", { name: "gpt-5.1" })).toBeNull();

  fireEvent.change(provider, { target: { value: "backup" } });
  expect((search as HTMLInputElement).value).toBe("");
  await screen.findByRole("button", { name: "claude-sonnet-5-5" });
  expect(screen.queryByRole("button", { name: "gpt-5.1-mini" })).toBeNull();
});

it("switches the session to the picked provider and model", async () => {
  const { picked, applied, opened, posts } = mount();
  const provider = await screen.findByRole("combobox", { name: t.provider });
  fireEvent.change(provider, { target: { value: "backup" } });
  await screen.findByRole("button", { name: "claude-sonnet-5-5" });

  fireEvent.click(screen.getByRole("button", { name: "claude-sonnet-5-5" }));

  await waitFor(() => expect(picked).toEqual([{ model: "claude-sonnet-5-5", providerId: "backup" }]));
  expect(posts).toHaveLength(1);
  expect(posts[0].url).toBe("/api/sessions/s-1/model");
  // Provider 一起下发：同名模型在不同网关下可能是两回事。
  expect(posts[0].body).toEqual({ model: "claude-sonnet-5-5", provider_id: "backup" });
  expect(screen.getByRole("status").textContent).toContain("已切换到 claude-sonnet-5-5");
  // 选完就收起面板，成功提示交给宿主常驻显示——否则提示跟着面板一起消失。
  // 初始渲染时 details 自己也会触发一次 toggle(true)，所以只看最后一次。
  expect(applied).toEqual(["已切换到 claude-sonnet-5-5"]);
  expect(opened.at(-1)).toBe(false);
});

it("reports a loopback-only refusal instead of pretending the switch happened", async () => {
  const { picked, applied, opened, posts } = mount({ post: () => Response.json({ error: "loopback only" }, { status: 403 }) });
  await screen.findByText("gpt-5.1");

  fireEvent.click(screen.getByRole("button", { name: "gpt-5.1" }));

  await screen.findByRole("alert");
  expect(screen.getByRole("alert").textContent).toContain(t.failed);
  expect(screen.getByRole("alert").textContent).toContain("loopback only");
  expect(picked).toEqual([]);
  expect(posts).toHaveLength(1);
  // 失败时面板留在原地：原因要看得见，也能马上重试。
  expect(applied).toEqual([]);
  expect(opened).not.toContain(false);
});

it("keeps the pick as a draft when the session does not exist yet", async () => {
  // 新会话还没有 id：选中只记在本地，随第一条消息一起交给服务端。
  // 不能为了选个模型先建一条空会话，那会在历史里留下一屏没人用过的条目。
  const { picked, applied, opened, posts } = mount({ sessionId: null });
  await screen.findByText("gpt-5.1");

  fireEvent.click(screen.getByRole("button", { name: "gpt-5.1-mini" }));

  await waitFor(() => expect(picked).toEqual([{ model: "gpt-5.1-mini", providerId: "main" }]));
  expect(posts).toHaveLength(0);
  expect(applied).toEqual(["已切换到 gpt-5.1-mini"]);
  expect(opened.at(-1)).toBe(false);
});

it("shows which model the first turn will use before any session exists", async () => {
  const { container } = mount({ sessionId: null, default: { model: { provider_id: "main", provider: "OpenAI 兼容网关", model: "gpt-5.1" }, writable: true } });
  // 按钮上直接写模型名，而不是留一句「选择模型」让人猜这一轮会跑什么。
  await waitFor(() => expect(container.querySelector("summary")?.textContent).toContain("gpt-5.1"));
  expect(screen.getByText(/默认 gpt-5\.1/)).toBeTruthy();
});

it("disables the options when the page cannot change models", async () => {
  mount({ sessionId: null, default: { model: null, writable: false } });
  await screen.findByText("gpt-5.1");
  await waitFor(() => expect((screen.getByRole("button", { name: "gpt-5.1" }) as HTMLButtonElement).disabled).toBe(true));
  expect(screen.getByText(t.notWritable)).toBeTruthy();
});
