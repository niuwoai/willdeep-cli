// @vitest-environment jsdom
import { afterEach, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { ChakraProvider, defaultSystem } from "@chakra-ui/react";
import { IntakeManager } from "./IntakeManager";
import { intakeText } from "./intakeMessages";
import type { IntakeView } from "./intakeApi";

afterEach(() => { cleanup(); vi.unstubAllGlobals(); });

const t = intakeText("zh-CN");

function view(over: Partial<IntakeView> = {}): IntakeView {
  return {
    enabled: false, listen: "127.0.0.1:9857", running: false, running_listen: null,
    runtime_managed: true, writable: true, sites: [], forms: [], webhooks: [], pending: [], bindings: [],
    ...over,
  };
}

function renderPage(onOpenSession = vi.fn()) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(<QueryClientProvider client={client}><ChakraProvider value={defaultSystem}><IntakeManager language="zh-CN" onOpenSession={onOpenSession} /></ChakraProvider></QueryClientProvider>);
  return onOpenSession;
}

it("saves a newly published site with the exact payload the intake service reads", async () => {
  const sent: unknown[] = [];
  vi.stubGlobal("fetch", vi.fn(async (_url: string, init?: RequestInit) => {
    if (init?.method === "PUT") {
      sent.push(JSON.parse(String(init.body)));
      return { ok: true, json: async () => view({ sites: [{ slug: "docs", root: "/Users/rocky/Sites/docs" }] }) };
    }
    return { ok: true, json: async () => view() };
  }));
  renderPage();
  await screen.findByText(t.title);
  fireEvent.click(screen.getByRole("button", { name: t.addSite }));
  fireEvent.change(screen.getByPlaceholderText("docs"), { target: { value: "docs" } });
  fireEvent.change(screen.getByPlaceholderText(t.rootPlaceholder), { target: { value: "/Users/rocky/Sites/docs" } });
  const save = screen.getByRole("button", { name: t.save }) as HTMLButtonElement;
  expect(save.disabled).toBe(false);
  fireEvent.click(save);
  await waitFor(() => expect(sent).toEqual([{
    enabled: false, listen: "127.0.0.1:9857",
    sites: [{ slug: "docs", root: "/Users/rocky/Sites/docs" }], forms: [], webhooks: [],
  }]));
  await screen.findByRole("status");
});

it("blocks saving from a non-loopback page and says so", async () => {
  vi.stubGlobal("fetch", vi.fn(async () => ({ ok: true, json: async () => view({ writable: false }) })));
  renderPage();
  await screen.findByText(t.notWritable);
  expect((screen.getByRole("button", { name: t.save }) as HTMLButtonElement).disabled).toBe(true);
});

it("lists bindings and failed deliveries, and opens the bound session", async () => {
  vi.stubGlobal("fetch", vi.fn(async () => ({
    ok: true,
    json: async () => view({
      bindings: [{ slug: "contact", session_id: "abc12345-1111-2222" }],
      pending: [{ at: 1_700_000_000, slug: "contact", kind: "form", reason: "workspace missing" }],
    }),
  })));
  const onOpenSession = renderPage();
  await screen.findByText(t.title);
  fireEvent.click(screen.getByRole("button", { name: t.records }));
  await screen.findByText("workspace missing");
  fireEvent.click(screen.getByRole("button", { name: t.sessionLink }));
  expect(onOpenSession).toHaveBeenCalledWith("abc12345-1111-2222");
  const table = screen.getByText("contact").closest("table") as HTMLTableElement;
  expect(within(table).getByText("abc12345")).toBeTruthy();
});

it("refuses to save two sites with the same slug before hitting the server", async () => {
  const calls: string[] = [];
  vi.stubGlobal("fetch", vi.fn(async (_url: string, init?: RequestInit) => {
    calls.push(init?.method ?? "GET");
    return { ok: true, json: async () => view({ sites: [{ slug: "docs", root: "/tmp/docs" }] }) };
  }));
  renderPage();
  await screen.findByText(t.title);
  fireEvent.click(screen.getByRole("button", { name: t.addSite }));
  fireEvent.change(screen.getAllByPlaceholderText("docs")[1], { target: { value: "docs" } });
  fireEvent.change(screen.getAllByPlaceholderText(t.rootPlaceholder)[1], { target: { value: "/tmp/docs-2" } });
  fireEvent.click(screen.getByRole("button", { name: t.save }));
  await screen.findByText(new RegExp(t.duplicateSlug));
  expect(calls).toEqual(["GET"]);
});
