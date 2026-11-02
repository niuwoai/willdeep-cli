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
    enabled: false, listen: "127.0.0.1:9857", public_url: "", running: false, running_listen: null,
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
    enabled: false, listen: "127.0.0.1:9857", public_url: "",
    sites: [{ slug: "docs", root: "/Users/rocky/Sites/docs" }], forms: [], webhooks: [],
  }]));
  await screen.findByRole("status");
});

it("saves the public address used for share and site links", async () => {
  const sent: unknown[] = [];
  vi.stubGlobal("fetch", vi.fn(async (_url: string, init?: RequestInit) => {
    if (init?.method === "PUT") {
      sent.push(JSON.parse(String(init.body)));
      return { ok: true, json: async () => view({ public_url: "https://wd.niuwoai.com", running: true, running_listen: "127.0.0.1:9857" }) };
    }
    return { ok: true, json: async () => view() };
  }));
  renderPage();
  await screen.findByText(t.title);
  fireEvent.change(screen.getByPlaceholderText("https://intake.example.com"), { target: { value: "https://wd.niuwoai.com" } });
  const save = screen.getByRole("button", { name: t.save }) as HTMLButtonElement;
  expect(save.disabled).toBe(false);
  fireEvent.click(save);
  await waitFor(() => expect((sent[0] as { public_url?: string }).public_url).toBe("https://wd.niuwoai.com"));
  await screen.findByRole("status");
});

it("rejects a public address with a path before hitting the server", async () => {
  const calls: string[] = [];
  vi.stubGlobal("fetch", vi.fn(async (_url: string, init?: RequestInit) => {
    calls.push(init?.method ?? "GET");
    return { ok: true, json: async () => view() };
  }));
  renderPage();
  await screen.findByText(t.title);
  fireEvent.change(screen.getByPlaceholderText("https://intake.example.com"), { target: { value: "https://example.com/intake" } });
  fireEvent.click(screen.getByRole("button", { name: t.save }));
  await screen.findByText(t.badPublicUrl);
  expect(calls).toEqual(["GET"]);
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

async function openFormsTab() {
  await screen.findByText(t.title);
  fireEvent.click(screen.getByRole("button", { name: t.forms }));
}

it("derives a unique form slug from the title and stops after an edit", async () => {
  vi.stubGlobal("fetch", vi.fn(async () => ({ ok: true, json: async () => view() })));
  renderPage();
  await openFormsTab();
  fireEvent.click(screen.getByRole("button", { name: t.addForm }));
  const title = screen.getByLabelText(t.formTitle);
  fireEvent.change(title, { target: { value: "Contact us" } });
  expect((screen.getByLabelText(t.slug) as HTMLInputElement).value).toBe("contact-us");
  fireEvent.change(title, { target: { value: "Support" } });
  expect((screen.getByLabelText(t.slug) as HTMLInputElement).value).toBe("support");

  // 手改之后不再跟着标题走；清空则回到自动，并立刻重算。
  fireEvent.change(screen.getByLabelText(t.slug), { target: { value: "help" } });
  fireEvent.change(title, { target: { value: "Contact us" } });
  expect((screen.getByLabelText(t.slug) as HTMLInputElement).value).toBe("help");
  fireEvent.change(screen.getByLabelText(t.slug), { target: { value: "" } });
  expect((screen.getByLabelText(t.slug) as HTMLInputElement).value).toBe("contact-us");
});

it("falls back to a placeholder slug when the title has no latin letters", async () => {
  vi.stubGlobal("fetch", vi.fn(async () => ({ ok: true, json: async () => view() })));
  renderPage();
  await openFormsTab();
  fireEvent.click(screen.getByRole("button", { name: t.addForm }));
  fireEvent.change(screen.getByLabelText(t.formTitle), { target: { value: "你的信息" } });
  expect((screen.getByLabelText(t.slug) as HTMLInputElement).value).toBe("form");
  expect(screen.getByText(t.slugAutoFallback)).toBeTruthy();
  fireEvent.click(screen.getByRole("button", { name: t.addForm }));
  fireEvent.change(screen.getAllByLabelText(t.formTitle)[1], { target: { value: "联系方式" } });
  expect((screen.getAllByLabelText(t.slug)[1] as HTMLInputElement).value).toBe("form-2");
});

it("keeps automatically derived slugs unique across forms", async () => {  vi.stubGlobal("fetch", vi.fn(async () => ({ ok: true, json: async () => view() })));
  renderPage();
  await openFormsTab();
  fireEvent.click(screen.getByRole("button", { name: t.addForm }));
  fireEvent.change(screen.getByLabelText(t.formTitle), { target: { value: "Contact us" } });
  fireEvent.click(screen.getByRole("button", { name: t.addForm }));
  fireEvent.change(screen.getAllByLabelText(t.formTitle)[1], { target: { value: "Contact us" } });
  const slugs = screen.getAllByLabelText(t.slug) as HTMLInputElement[];
  expect(slugs.map((input) => input.value)).toEqual(["contact-us", "contact-us-2"]);
});

it("names a field from its label, infers the type, and saves number bounds", async () => {
  const sent: unknown[] = [];
  vi.stubGlobal("fetch", vi.fn(async (_url: string, init?: RequestInit) => {
    if (init?.method === "PUT") {
      sent.push(JSON.parse(String(init.body)));
      return { ok: true, json: async () => view() };
    }
    return { ok: true, json: async () => view() };
  }));
  renderPage();
  await openFormsTab();
  fireEvent.click(screen.getByRole("button", { name: t.addForm }));
  fireEvent.change(screen.getByLabelText(t.formTitle), { target: { value: "Sign up" } });
  fireEvent.change(screen.getByLabelText(t.workspace), { target: { value: "/tmp/work" } });
  fireEvent.change(screen.getByLabelText(t.prompt), { target: { value: "收到一份报名" } });
  fireEvent.click(screen.getByRole("button", { name: t.addField }));

  fireEvent.change(screen.getByLabelText(t.fieldLabel), { target: { value: "年龄" } });
  const name = screen.getByLabelText(t.fieldName) as HTMLInputElement;
  expect(name.value).toBe("年龄");
  expect((screen.getByLabelText(t.fieldType) as HTMLSelectElement).value).toBe("number");
  expect(screen.getByLabelText(t.minValue)).toBeTruthy();

  fireEvent.click(screen.getByRole("checkbox", { name: t.allowDecimal }));
  fireEvent.change(screen.getByLabelText(t.minValue), { target: { value: "18" } });
  fireEvent.change(screen.getByLabelText(t.maxValue), { target: { value: "99" } });
  fireEvent.click(screen.getByRole("button", { name: t.save }));

  await waitFor(() => expect(sent).toHaveLength(1));
  const form = (sent[0] as { forms: Array<Record<string, unknown>> }).forms[0];
  expect(form.slug).toBe("sign-up");
  expect(form.fields).toEqual([{
    name: "年龄", label: "年龄", type: "number", required: false, options: [],
    allow_decimal: true, min: 18, max: 99,
  }]);
});

it("keeps automatically derived field names unique", async () => {
  vi.stubGlobal("fetch", vi.fn(async () => ({ ok: true, json: async () => view() })));
  renderPage();
  await openFormsTab();
  fireEvent.click(screen.getByRole("button", { name: t.addForm }));
  fireEvent.click(screen.getByRole("button", { name: t.addField }));
  fireEvent.click(screen.getByRole("button", { name: t.addField }));
  const labels = screen.getAllByLabelText(t.fieldLabel);
  fireEvent.change(labels[0], { target: { value: "邮箱" } });
  fireEvent.change(labels[1], { target: { value: "邮箱" } });
  const names = screen.getAllByLabelText(t.fieldName) as HTMLInputElement[];
  expect(names.map((input) => input.value)).toEqual(["邮箱", "邮箱-2"]);
});

it("blocks a number field whose minimum is greater than its maximum", async () => {
  const calls: string[] = [];
  vi.stubGlobal("fetch", vi.fn(async (_url: string, init?: RequestInit) => {
    calls.push(init?.method ?? "GET");
    return { ok: true, json: async () => view() };
  }));
  renderPage();
  await openFormsTab();
  fireEvent.click(screen.getByRole("button", { name: t.addForm }));
  fireEvent.change(screen.getByLabelText(t.formTitle), { target: { value: "Sign up" } });
  fireEvent.change(screen.getByLabelText(t.workspace), { target: { value: "/tmp/work" } });
  fireEvent.change(screen.getByLabelText(t.prompt), { target: { value: "收到一份报名" } });
  fireEvent.click(screen.getByRole("button", { name: t.addField }));
  fireEvent.change(screen.getByLabelText(t.fieldLabel), { target: { value: "年龄" } });
  fireEvent.change(screen.getByLabelText(t.minValue), { target: { value: "10" } });
  fireEvent.change(screen.getByLabelText(t.maxValue), { target: { value: "1" } });
  fireEvent.click(screen.getByRole("button", { name: t.save }));
  await screen.findByText(new RegExp(t.badNumberRange));
  expect(calls).toEqual(["GET"]);
});
