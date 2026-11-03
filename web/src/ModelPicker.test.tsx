// @vitest-environment jsdom
import { afterEach, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { ChakraProvider, defaultSystem } from "@chakra-ui/react";
import { ModelPicker } from "./ModelPicker";
import { modelText } from "./modelMessages";

afterEach(() => { cleanup(); vi.unstubAllGlobals(); });

const t = modelText("zh-CN");

const OPTIONS = [
  { provider_id: "main", provider: "OpenAI 兼容网关", model: "gpt-5.1" },
  { provider_id: "main", provider: "OpenAI 兼容网关", model: "gpt-5.1-mini" },
  { provider_id: "backup", provider: "备用网关", model: "claude-sonnet-5-5" },
];

function mount(over: { current?: string | null; post?: (url: string, init?: RequestInit) => Response } = {}) {
  const picked: string[] = [];
  const applied: string[] = [];
  const opened: boolean[] = [];
  const posts: { url: string; body: unknown }[] = [];
  vi.stubGlobal("fetch", vi.fn(async (url: string, init?: RequestInit) => {
    if (init?.method === "POST") {
      posts.push({ url, body: JSON.parse(String(init.body)) });
      return over.post ? over.post(url, init) : Response.json({});
    }
    return Response.json(OPTIONS);
  }));
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const view = render(<QueryClientProvider client={client}>
    <ChakraProvider value={defaultSystem}>
      <ModelPicker
        sessionId="s-1" language="zh-CN" current={over.current ?? null} open
        onOpenChange={(open) => { opened.push(open); }}
        onPicked={(model) => { picked.push(model); }}
        onApplied={(text) => { applied.push(text); }}
      />
    </ChakraProvider>
  </QueryClientProvider>);
  return { ...view, picked, applied, opened, posts };
}

it("groups the configured models by provider and marks the session's current one", async () => {
  const { container } = mount({ current: "gpt-5.1" });
  await screen.findByText("claude-sonnet-5-5");

  const titles = [...container.querySelectorAll(".model-group-title")].map((node) => node.textContent);
  expect(titles).toHaveLength(2);
  expect(titles[0]).toContain("main");
  expect(titles[1]).toContain("backup");

  const current = screen.getByRole("button", { name: "gpt-5.1" });
  expect(current.getAttribute("aria-pressed")).toBe("true");
});

it("switches the session to the picked provider and model", async () => {
  const { picked, applied, opened, posts } = mount();
  await screen.findByText("claude-sonnet-5-5");

  fireEvent.click(screen.getByRole("button", { name: "claude-sonnet-5-5" }));

  await waitFor(() => expect(picked).toEqual(["claude-sonnet-5-5"]));
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
