// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { PermissionsPanel } from "./PermissionsPanel";
import { messages } from "./i18n";
import type { WorkspaceAccess } from "./accessModes";

afterEach(() => { cleanup(); vi.unstubAllGlobals(); });

function mount(props: { override?: WorkspaceAccess | null; workspaceAccess?: WorkspaceAccess; sessionId?: string | null } = {}) {
  const saved: WorkspaceAccess[] = [];
  const applied: string[] = [];
  const opened: boolean[] = [];
  const requests: { url: string; body: unknown }[] = [];
  vi.stubGlobal("fetch", vi.fn(async (url: string, init?: RequestInit) => {
    requests.push({ url, body: JSON.parse(String(init?.body)) });
    return Response.json({});
  }));
  const view = render(<PermissionsPanel
    sessionId={props.sessionId === undefined ? "s-1" : props.sessionId}
    language="zh-CN"
    messages={messages["zh-CN"]}
    override={props.override ?? null}
    workspaceAccess={props.workspaceAccess ?? "smart"}
    open={false}
    onOpenChange={(open) => { opened.push(open); }}
    onSaved={(mode) => { saved.push(mode); }}
    onApplied={(text) => { applied.push(text); }}
  />);
  return { ...view, saved, applied, opened, requests };
}

describe("permissions panel", () => {
  it("shows the effective mode and applies a plain switch", async () => {
    const { container, saved, applied, opened, requests } = mount();
    expect(container.textContent).toContain("跟随工作区");
    // 还没改草稿时应用按钮没有意义。
    const apply = screen.getByRole("button", { name: "应用" });
    expect((apply as HTMLButtonElement).disabled).toBe(true);

    fireEvent.click(screen.getByRole("radio", { name: /^严格/ }));
    fireEvent.click(screen.getByRole("button", { name: "应用" }));

    await waitFor(() => expect(saved).toEqual(["strict"]));
    expect(requests).toHaveLength(1);
    expect(requests[0].url).toBe("/api/sessions/s-1/approval-mode");
    expect(requests[0].body).toEqual({ approval_mode: "strict", confirm: false });
    // 应用成功就收起面板，成功提示交给宿主常驻几秒。
    // 初始渲染时 details 自己也会触发一次 toggle(true)，所以只看最后一次。
    expect(applied).toEqual(["已切换到严格，下一轮生效"]);
    expect(opened.at(-1)).toBe(false);
  });

  it("refuses to raise full access until the operator acknowledges it", async () => {
    const { saved, requests } = mount({ workspaceAccess: "workspace_write" });
    fireEvent.click(screen.getByRole("radio", { name: /^完全访问/ }));

    const apply = screen.getByRole("button", { name: "应用" }) as HTMLButtonElement;
    expect(apply.disabled).toBe(true);
    expect(screen.getByText(/跳过工作区内的写入与命令审批/)).toBeTruthy();

    fireEvent.click(screen.getByRole("checkbox", { name: /我知道/ }));
    await waitFor(() => expect(apply.disabled).toBe(false));
    fireEvent.click(apply);

    await waitFor(() => expect(saved).toEqual(["full_access"]));
    expect(requests[0].body).toEqual({ approval_mode: "full_access", confirm: true });
  });

  it("cannot override a read-only workspace", () => {
    const { container } = mount({ workspaceAccess: "read_only" });
    expect(screen.getByText(/这个工作区是只读策略/)).toBeTruthy();
    // 整组选项一起锁住：fieldset 的 disabled 会连带收起里面每一个 radio。
    expect((container.querySelector("fieldset") as HTMLFieldSetElement).disabled).toBe(true);
    expect((screen.getByRole("button", { name: "应用" }) as HTMLButtonElement).disabled).toBe(true);
    expect(screen.queryByText(/跳过工作区内的写入与命令审批/)).toBeNull();
  });

  it("explains that a non-loopback page cannot write settings", async () => {
    const saved: WorkspaceAccess[] = [];
    const applied: string[] = [];
    const opened: boolean[] = [];
    vi.stubGlobal("fetch", vi.fn(async () => Response.json({ error: "loopback only" }, { status: 403 })));
    render(<PermissionsPanel
      sessionId="s-1" language="zh-CN" messages={messages["zh-CN"]} override={null} workspaceAccess="smart"
      open onOpenChange={(open) => { opened.push(open); }} onSaved={(mode) => { saved.push(mode); }}
      onApplied={(text) => { applied.push(text); }}
    />);
    fireEvent.click(screen.getByRole("radio", { name: /^严格/ }));
    fireEvent.click(screen.getByRole("button", { name: "应用" }));
    expect(await screen.findByRole("alert")).toBeTruthy();
    expect(screen.getByRole("alert").textContent).toContain("只有在本机回环地址");
    expect(saved).toEqual([]);
    // 失败不收起面板，也不报成功。
    expect(applied).toEqual([]);
    expect(opened).not.toContain(false);
  });

  it("keeps the choice in the draft when the session does not exist yet", async () => {
    // 新会话还没发第一条消息时，档位只能先记在本地：为此建一条空会话只会在
    // 历史里留一屏没人用过的条目。
    const { saved, applied, opened, requests } = mount({ sessionId: null, workspaceAccess: "workspace_write" });
    fireEvent.click(screen.getByRole("radio", { name: /^严格/ }));
    fireEvent.click(screen.getByRole("button", { name: "应用" }));

    await waitFor(() => expect(saved).toEqual(["strict"]));
    expect(requests).toHaveLength(0);
    expect(applied).toEqual(["已切换到严格，下一轮生效"]);
    expect(opened.at(-1)).toBe(false);
  });
});
