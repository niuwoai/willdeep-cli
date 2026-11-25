// @vitest-environment jsdom
import { useState } from "react";
import { cleanup, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, expect, it } from "vitest";
import { GoalModeControl } from "./GoalModeControl";
import { messages } from "./i18n";

const t = messages["zh-CN"];

function Harness({ initialGoal = "", initiallyEnabled = false }: { initialGoal?: string; initiallyEnabled?: boolean }) {
  const [preference, setPreference] = useState({ goal: initialGoal, enabled: initiallyEnabled });
  return <>
    <GoalModeControl {...preference} messages={t} onChange={(goal, enabled) => setPreference({ goal, enabled })} />
    <button type="button">聊天输入</button>
  </>;
}

afterEach(cleanup);

it("仅开启目标模式后才显示目标，关闭再开启可恢复原目标", async () => {
  const user = userEvent.setup();
  render(<Harness initialGoal="我的目标" />);
  const toggle = screen.getByRole("checkbox", { name: t.goalMode });
  expect(screen.queryByText("我的目标")).toBeNull();
  expect(screen.queryByRole("textbox")).toBeNull();
  expect(screen.queryByRole("button", { name: t.goalModeEdit })).toBeNull();
  await user.click(toggle);
  expect(screen.getByText("我的目标")).toBeTruthy();
  expect(screen.getByRole("button", { name: t.goalModeEdit })).toBeTruthy();
  expect(screen.queryByRole("textbox")).toBeNull();
  await user.click(toggle);
  expect(screen.queryByText("我的目标")).toBeNull();
});

it("空目标可输入多行，离开输入框后显示全文和编辑按钮", async () => {
  const user = userEvent.setup();
  render(<Harness initiallyEnabled />);
  const editor = screen.getByRole("textbox", { name: t.goalMode });
  expect(editor).toBeInstanceOf(HTMLTextAreaElement);
  await user.type(editor, "第一行{Enter}第二行");
  expect(screen.getByRole("textbox", { name: t.goalMode })).toBe(editor);
  await user.click(screen.getByRole("button", { name: "聊天输入" }));
  expect(screen.queryByRole("textbox")).toBeNull();
  expect(screen.getByText("第一行\n第二行", { normalizer: (text) => text })).toBeTruthy();
  await user.click(screen.getByRole("button", { name: t.goalModeEdit }));
  const reopened = screen.getByRole<HTMLTextAreaElement>("textbox", { name: t.goalMode });
  expect(document.activeElement).toBe(reopened);
  expect(reopened.value).toBe("第一行\n第二行");
  await user.clear(reopened);
  await user.click(screen.getByRole("button", { name: "聊天输入" }));
  expect(screen.getByRole("textbox", { name: t.goalMode })).toBe(reopened);
  expect(screen.queryByRole("button", { name: t.goalModeEdit })).toBeNull();
});
