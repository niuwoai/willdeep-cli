// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { RunCard, ThoughtCard } from "./RunCard";
import { messages } from "./i18n";
import { groupActivityMessages, type RunStep } from "./runActivity";

afterEach(cleanup);
const t = messages["zh-CN"];
function tool(index: number, status: RunStep["status"] = "done"): RunStep {
  return { id: `tool-${index}`, label: `${status} run_command`, detail: `command ${index}\nargument ${index}`, status, startedAt: 0, elapsedMs: 1500 };
}

describe("连续工具调用", () => {
  it("七条独立历史卡片合并为一行，展开恢复全部命令和耗时", () => {
    const input = Array.from({ length: 7 }, (_, i) => ({ id: `message-${i}`, role: "activity", content: "", steps: [tool(i)] }));
    const grouped = groupActivityMessages(input);
    expect(grouped).toHaveLength(1);
    expect(grouped[0].id).toBe("message-0");
    expect(input.every(message => message.steps.length === 1)).toBe(true);
    const { container } = render(<RunCard steps={grouped[0].steps} messages={t} />);
    expect(container.querySelectorAll(".run-step")).toHaveLength(1);
    expect(screen.getByText("7 次工具调用")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: t.expand }));
    expect(container.querySelectorAll(".run-details .run-step")).toHaveLength(7);
    for (let i = 0; i < 7; i++) expect(screen.getByTitle(`command ${i} argument ${i}`).getAttribute("title")).toBe(`command ${i}\nargument ${i}`);
    expect(screen.getAllByText("1.5s")).toHaveLength(7);
    fireEvent.click(screen.getByRole("button", { name: t.collapse }));
    expect(container.querySelectorAll(".run-step")).toHaveLength(1);
  });

  it("正文、用户消息和思考形成边界，不把后面的调用挪到正文前面", () => {
    const activity = (id: string, content = "", thinking = "") => ({ id, role: "activity", content, thinking, steps: [tool(1)] });
    const input: { id: string; role: string; content: string; steps?: RunStep[]; thinking?: string }[] = [activity("a"), activity("b", "正文"), activity("c"), { id: "user", role: "user", content: "继续" }, activity("d", "", "思考"), activity("e")];
    const grouped = groupActivityMessages(input);
    expect(grouped.map(message => message.id)).toEqual(["a", "c", "user", "d", "e"]);
    expect(grouped[0].content).toBe("正文");
    expect(grouped[0].steps).toHaveLength(2);
    expect(input[0].content).toBe("");
  });

  it("运行中和失败状态最多两行，失败不会被完成步骤遮住", () => {
    const steps = [tool(0, "failed"), tool(1), tool(2, "active")];
    const { container, rerender } = render(<RunCard steps={steps} messages={t} />);
    expect(container.querySelectorAll(".run-step")).toHaveLength(2);
    expect(screen.getByText("3 次工具调用 · 1 失败")).toBeTruthy();
    expect(screen.getByTitle("command 2 argument 2")).toBeTruthy();
    rerender(<RunCard steps={steps.map(step => step.status === "active" ? { ...step, status: "done" } : step)} messages={t} />);
    expect(container.querySelectorAll(".run-step")).toHaveLength(2);
    expect(screen.getByTitle("command 0 argument 0")).toBeTruthy();
  });

  it("后续实时更新不关闭展开状态，思考轮次不计入工具次数", () => {
    const steps: RunStep[] = [{ id: "turn-1", label: t.thinking, status: "done", startedAt: 0 }, tool(1)];
    const { rerender } = render(<RunCard steps={steps} messages={t} />);
    fireEvent.click(screen.getByRole("button", { name: t.expand }));
    rerender(<RunCard steps={[...steps, tool(2, "active")]} messages={t} />);
    expect(screen.getByText("2 次工具调用")).toBeTruthy();
    expect(screen.getByRole("button", { name: t.collapse }).getAttribute("aria-expanded")).toBe("true");
    expect(screen.getByTitle("command 2 argument 2")).toBeTruthy();
  });

  it("思考默认只显示耗时，点击才展开正文", () => {
    render(<ThoughtCard thought={{ text: "private reasoning", startedAt: 0, elapsedMs: 1800 }} messages={t} />);
    expect(screen.getByText("思考 1.8s")).toBeTruthy();
    expect(screen.queryByText("private reasoning")).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: /思考 1.8s/ }));
    expect(screen.getByText("private reasoning")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: /思考 1.8s/ }));
    expect(screen.queryByText("private reasoning")).toBeNull();
  });

  it("不足一秒时显示毫秒", () => {
    render(<ThoughtCard thought={{ text: "fast", startedAt: 0, elapsedMs: 240 }} messages={t} />);
    expect(screen.getByText("思考 240ms")).toBeTruthy();
  });

  for (const [language, dictionary] of Object.entries(messages)) it(`${language} 的摘要和操作来自字典`, () => {
    render(<RunCard steps={[tool(1), tool(2)]} messages={dictionary} />);
    expect(screen.getByText(dictionary.toolActivitySummary.replace("{count}", "2"))).toBeTruthy();
    expect(screen.getByRole("button", { name: dictionary.expand })).toBeTruthy();
  });
});
