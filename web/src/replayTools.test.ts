import { expect, it } from "vitest";
import { replayToolState } from "./replayTools";
import { messages } from "./i18n";

it("does not label a suspended question or approval as failed after reconnect", () => {
  for (const name of ["ask_user", "run_command"]) {
    const state = replayToolState(false, name, messages["zh-CN"]);
    expect(state.status).toBe("pending");
    expect(state.label).not.toContain(messages["zh-CN"].interrupted);
  }
});

it("keeps completed historical tools completed", () => {
  expect(replayToolState(true, "ask_user", messages.en)).toEqual({ label: "Done ask_user", status: "done" });
});
