// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { InstanceIdentitySettings } from "./InstanceIdentitySettings";
import { identityText } from "./identityMessages";
import type { Language } from "./i18n";
import type { IdentityView } from "./identityApi";

const current: IdentityView = { id: "6f0a1111-2222-3333-4444-55550abc1234", name: "willdeep", created_at: 7 };
const clients: QueryClient[] = [];

afterEach(() => {
  cleanup();
  clients.forEach((client) => client.clear());
  clients.length = 0;
  vi.unstubAllGlobals();
});

function mount(language: Language = "zh-CN") {
  const onIdentityChange = vi.fn();
  const client = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } });
  clients.push(client);
  render(<QueryClientProvider client={client}><InstanceIdentitySettings language={language} onIdentityChange={onIdentityChange} /></QueryClientProvider>);
  return { client, onIdentityChange };
}

const nameInput = () => screen.getByRole<HTMLInputElement>("textbox", { name: "实例名" });
const saveButton = () => screen.getByRole<HTMLButtonElement>("button", { name: "保存名字" });

describe("instance identity settings", () => {
  it("reads the current name on mount and prevents saving while loading or unchanged", async () => {
    let finishLoad!: (response: Response) => void;
    const fetchMock = vi.fn(() => new Promise<Response>((resolve) => { finishLoad = resolve; }));
    vi.stubGlobal("fetch", fetchMock);
    mount();

    expect(screen.getByRole("status").textContent).toBe("正在读取实例名…");
    expect(nameInput().disabled).toBe(true);
    expect(saveButton().disabled).toBe(true);
    expect(fetchMock).toHaveBeenCalledWith("/api/identity", { signal: expect.any(AbortSignal) });

    await act(async () => finishLoad(Response.json(current)));
    await waitFor(() => expect(nameInput().value).toBe(current.name));
    expect(nameInput().disabled).toBe(false);
    expect(saveButton().disabled).toBe(true);
    expect(screen.queryByRole("status")).toBeNull();

    fireEvent.change(nameInput(), { target: { value: "   " } });
    expect(saveButton().disabled).toBe(true);
  });

  it.each(["小滚", "こまち"])("saves %s through the shared API and returns the unchanged UUID to the host", async (name) => {
    const next: IdentityView = { ...current, name, renamed_at: 9 };
    const fetchMock = vi.fn(async (url: string) => Response.json(url === "/api/identity" ? current : next));
    vi.stubGlobal("fetch", fetchMock);
    const { client, onIdentityChange } = mount();
    await waitFor(() => expect(nameInput().value).toBe(current.name));

    fireEvent.change(nameInput(), { target: { value: name } });
    fireEvent.click(saveButton());

    await waitFor(() => expect(onIdentityChange).toHaveBeenCalledExactlyOnceWith(next));
    expect(fetchMock).toHaveBeenLastCalledWith("/api/identity/name", {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ name }),
    });
    expect(onIdentityChange.mock.calls[0][0].id).toBe(current.id);
    expect(client.getQueryData(["instance-identity"])).toEqual(next);
    expect(nameInput().value).toBe(name);
    expect(saveButton().disabled).toBe(true);
    expect(screen.getByRole("status").textContent).toBe("实例名已保存");
  });

  it("keeps the draft and server validation error after a failed save, then allows correction", async () => {
    const fetchMock = vi.fn(async (url: string, init?: RequestInit) => {
      if (url === "/api/identity") return Response.json(current);
      const { name } = JSON.parse(String(init?.body)) as { name: string };
      return name === "小滚!" ? Response.json({ error: "name contains an unsupported character" }, { status: 400 })
        : Response.json({ ...current, name });
    });
    vi.stubGlobal("fetch", fetchMock);
    const { onIdentityChange } = mount();
    await waitFor(() => expect(nameInput().value).toBe(current.name));

    fireEvent.change(nameInput(), { target: { value: "小滚!" } });
    fireEvent.click(saveButton());
    expect((await screen.findByRole("alert")).textContent).toContain("保存实例名失败: name contains an unsupported character");
    expect(nameInput().value).toBe("小滚!");
    expect(nameInput().disabled).toBe(false);
    expect(onIdentityChange).not.toHaveBeenCalled();
    expect(screen.queryByRole("status")).toBeNull();

    fireEvent.change(nameInput(), { target: { value: "小滚" } });
    expect(screen.queryByRole("alert")).toBeNull();
    fireEvent.click(saveButton());
    await waitFor(() => expect(onIdentityChange).toHaveBeenCalledExactlyOnceWith({ ...current, name: "小滚" }));
  });

  it("prevents saving after a failed load and supports reloading", async () => {
    const fetchMock = vi.fn().mockResolvedValueOnce(Response.json({ error: "identity unavailable" }, { status: 500 }))
      .mockResolvedValueOnce(Response.json(current));
    vi.stubGlobal("fetch", fetchMock);
    const { onIdentityChange } = mount();

    expect((await screen.findByRole("alert")).textContent).toContain("读取实例名失败: identity unavailable");
    expect(nameInput().disabled).toBe(true);
    expect(saveButton().disabled).toBe(true);
    fireEvent.submit(screen.getByRole("form", { name: "实例名" }));
    expect(fetchMock).toHaveBeenCalledTimes(1);
    expect(onIdentityChange).not.toHaveBeenCalled();

    fireEvent.click(screen.getByRole("button", { name: "重新读取" }));
    await waitFor(() => expect(nameInput().value).toBe(current.name));
    expect(nameInput().disabled).toBe(false);
    expect(screen.queryByRole("alert")).toBeNull();
  });

  it("explains the loopback restriction when the server denies identity access", async () => {
    vi.stubGlobal("fetch", vi.fn(async () => Response.json({ error: "loopback only" }, { status: 403 })));
    const { onIdentityChange } = mount();

    expect((await screen.findByRole("alert")).textContent).toBe("请通过本机回环地址打开 Web，才能查看和修改实例名。");
    expect(nameInput().disabled).toBe(true);
    expect(saveButton().disabled).toBe(true);
    expect(onIdentityChange).not.toHaveBeenCalled();
  });

  it("disables editing and prevents repeated submissions while a save is pending", async () => {
    let finishSave!: (response: Response) => void;
    const fetchMock = vi.fn((url: string) => url === "/api/identity" ? Promise.resolve(Response.json(current))
      : new Promise<Response>((resolve) => { finishSave = resolve; }));
    vi.stubGlobal("fetch", fetchMock);
    const { onIdentityChange } = mount();
    await waitFor(() => expect(nameInput().value).toBe(current.name));
    fireEvent.change(nameInput(), { target: { value: "小滚" } });
    fireEvent.click(saveButton());

    const saving = await screen.findByRole<HTMLButtonElement>("button", { name: "正在保存…" });
    expect(saving.disabled).toBe(true);
    expect(nameInput().disabled).toBe(true);
    fireEvent.click(saving);
    fireEvent.submit(screen.getByRole("form", { name: "实例名" }));
    expect(fetchMock).toHaveBeenCalledTimes(2);
    expect(onIdentityChange).not.toHaveBeenCalled();

    await act(async () => finishSave(Response.json({ ...current, name: "小滚" })));
    await waitFor(() => expect(onIdentityChange).toHaveBeenCalledTimes(1));
  });

  it.each(["en", "ja"] as const)("uses %s messages without changing the identity", async (language) => {
    vi.stubGlobal("fetch", vi.fn(async () => Response.json(current)));
    const { onIdentityChange } = mount(language);
    const t = identityText(language);
    await waitFor(() => expect(screen.getByRole<HTMLInputElement>("textbox", { name: t.title }).value).toBe(current.name));
    expect(screen.getByText(t.description)).toBeTruthy();
    expect(screen.getByText(t.nameHint)).toBeTruthy();
    expect(screen.getByRole<HTMLButtonElement>("button", { name: t.save }).disabled).toBe(true);
    expect(onIdentityChange).not.toHaveBeenCalled();
  });
});
