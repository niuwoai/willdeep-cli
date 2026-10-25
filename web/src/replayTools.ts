import type { Messages } from "./i18n";
import type { RunStep } from "./runActivity";

/** Missing tool output can mean a live human wait; replay cannot infer failure. */
export function replayToolState(completed: boolean, name: string, t: Messages): Pick<RunStep, "label" | "status"> {
  return completed
    ? { label: `${t.toolDone} ${name}`, status: "done" }
    : { label: `${t.toolResultPending} ${name}`, status: "pending" };
}
