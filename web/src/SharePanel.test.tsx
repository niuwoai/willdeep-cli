// @vitest-environment jsdom
import { afterEach, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { ChakraProvider, defaultSystem } from "@chakra-ui/react";
import { SharePanel } from "./SharePanel";
import { shareText } from "./shareMessages";
import type { IntakeView } from "./intakeApi";

afterEach(() => { cleanup(); vi.unstubAllGlobals(); });

const t = shareText("zh-CN");

function intake(over: Partial<IntakeView> = {}): IntakeView {
  return {
    enabled: true, listen: "0.0.0.0:9857", running: true, running_listen: "0.0.0.0:9857",
    runtime_managed: true, writable: true, sites: [], forms: [], webhooks: [], pending: [], bindings: [],
    ...over,
  };
}

/// 一个只看两个 GET 的后端替身：分享状态与公网入口状态。
function stub(over: { share?: unknown; intake?: Partial<IntakeView>; onWrite?: (init?: RequestInit) => Response } = {}) {
  const writes: { method: string; url: string; body: unknown }[] = [];
  vi.stubGlobal("fetch", vi.fn(async (url: string, init?: RequestInit) => {
    if (init?.method && init.method !== "GET") {
      const body = init.body ? JSON.parse(String(init.body)) : null;
      writes.push({ method: init.method, url, body });
      return over.onWrite ? over.onWrite(init) : Response.json({ share: null });
    }
    if (url.includes("/share")) return Response.json({ share: over.share ?? null });
    return Response.json(intake(over.intake));
  }));
  return writes;
}

function mount() {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const onOpenChange = vi.fn();
  render(<QueryClientProvider client={client}>
    <ChakraProvider value={defaultSystem}>
      <SharePanel sessionId="s-1" language="zh-CN" open onOpenChange={onOpenChange} />
    </ChakraProvider>
  </QueryClientProvider>);
  return onOpenChange;
}

it("will not hand out a chat share until the operator acknowledges it", async () => {
  const writes = stub();
  mount();
  await screen.findByText(t.read);

  fireEvent.click(screen.getByRole("radio", { name: new RegExp(`^${t.chat}`) }));
  const create = screen.getByRole("button", { name: t.create }) as HTMLButtonElement;
  expect(create.disabled).toBe(true);
  expect(screen.getByText(t.chatWarning)).toBeTruthy();

  fireEvent.click(screen.getByRole("checkbox"));
  await waitFor(() => expect(create.disabled).toBe(false));
  fireEvent.click(create);

  await waitFor(() => expect(writes).toHaveLength(1));
  expect(writes[0]).toMatchObject({ method: "POST", url: "/api/sessions/s-1/share", body: { mode: "chat" } });
});

it("turns the token into a link on the public intake listener and stops sharing", async () => {
  const writes = stub({
    share: { token: "abc123", mode: "read", created_at: 1_700_000_000, path: "/share/abc123" },
    intake: { running: true, running_listen: "0.0.0.0:9857" },
  });
  mount();

  const link = await screen.findByRole("link");
  expect(link.getAttribute("href")).toBe(`http://${window.location.hostname}:9857/share/abc123`);
  expect(screen.getByText(t.read)).toBeTruthy();

  fireEvent.click(screen.getByRole("button", { name: t.stop }));
  await waitFor(() => expect(writes).toHaveLength(1));
  expect(writes[0]).toMatchObject({ method: "DELETE", url: "/api/sessions/s-1/share" });
});

it("says the link cannot be opened while the intake listener is down", async () => {
  stub({ share: { token: "abc123", mode: "read", created_at: 1_700_000_000, path: "/share/abc123" }, intake: { running: false, running_listen: null } });
  mount();

  // 顶部提示与链接位置各说一次：链接存在，但入口没跑就打不开。
  expect((await screen.findAllByText(t.needsIntake)).length).toBeGreaterThan(0);
  expect(screen.queryByRole("link")).toBeNull();
  expect((screen.getByRole("button", { name: t.copy }) as HTMLButtonElement).disabled).toBe(true);
});
