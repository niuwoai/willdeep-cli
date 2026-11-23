/**
 * Contract for the shell hooks configured in willdeep-cli.
 *
 * The host owns registration, ordering, timeouts, and process execution.
 * A TypeScript program consumes the one-line JSON object written to stdin
 * and communicates only through its exit status and stderr. This module has
 * no registration API because adding one would imply a capability the host
 * does not provide.
 */

/** Events the host can deliver. Names match `[[hooks]].event` exactly. */
export const HOOK_EVENTS = [
  "before_agent",
  "agent_end",
  "pre_tool",
  "post_tool",
  "approval_resolved",
] as const;

export type HookEventName = (typeof HOOK_EVENTS)[number];

/** Events on which `blocking = true` is accepted and a nonzero exit can deny. */
export const BLOCKING_HOOK_EVENTS = ["before_agent", "pre_tool"] as const;

export type BlockingHookEventName = (typeof BLOCKING_HOOK_EVENTS)[number];

/** Events whose action has already happened; blocking configuration is rejected. */
export const OBSERVE_ONLY_HOOK_EVENTS = [
  "agent_end",
  "post_tool",
  "approval_resolved",
] as const;

export type ObserveOnlyHookEventName = (typeof OBSERVE_ONLY_HOOK_EVENTS)[number];

/** What the host does when a blocking hook cannot return a judgment. */
export const HOOK_FAILURE_POLICIES = ["deny", "ignore"] as const;

export type HookFailurePolicy = (typeof HOOK_FAILURE_POLICIES)[number];

/**
 * Real stop reasons for one agent run, plus the two outcomes supplied only
 * when the run did not produce an `AgentStopReason`.
 */
export const AGENT_END_OUTCOMES = [
  "finished",
  "goal_complete",
  "budget_limited",
  "max_turns",
  "incomplete",
  "unverified",
  "error",
  "blocked",
] as const;

export type AgentEndOutcome = (typeof AGENT_END_OUTCOMES)[number];

export const TOOL_OUTCOMES = ["ok", "error"] as const;

export type ToolOutcome = (typeof TOOL_OUTCOMES)[number];

/** Decisions returned by the approver. `deny` is not specifically a human denial. */
export const APPROVER_OUTCOMES = ["allow_once", "always_allow", "deny"] as const;

export type ApproverOutcome = (typeof APPROVER_OUTCOMES)[number];

/** Approval modes recorded when the decision is delivered. */
export const APPROVAL_MODES = [
  "read-only",
  "strict",
  "smart",
  "workspace-write",
  "full-access",
] as const;

export type ApprovalMode = (typeof APPROVAL_MODES)[number];

/** A hook can inspect the event, but the host does not accept rewritten arguments. */
export type HookEffect = "allow" | "deny";

interface HookEventBase {
  /** Bound by the host registry. Null when the call has no session. */
  readonly session_id: string | null;
  /** Bound by the host registry. */
  readonly workspace: string | null;
  /**
   * Host-defined annotations. `truncated` is `"true"` when any field was
   * shortened to keep the frame within 16 KiB. A truncated value is not the
   * complete command or diff. The field is absent when there are no annotations.
   */
  readonly extra?: Readonly<Record<string, string>>;
}

/** Delivered before model processing. `tool` and `detail` are null. */
export interface BeforeAgentEvent extends HookEventBase {
  readonly event: "before_agent";
  readonly tool: null;
  readonly detail: null;
  readonly outcome: null;
  readonly exit_code: null;
}

/**
 * Delivered when the agent run returns. It observes the outcome and cannot
 * change it. Delivery is not retried after cancellation or process exit, and
 * it does not mean background work has finished.
 */
export interface AgentEndEvent extends HookEventBase {
  readonly event: "agent_end";
  readonly tool: null;
  readonly detail: null;
  readonly outcome: AgentEndOutcome;
  readonly exit_code: null;
}

/** Delivered at the single execution choke point, before a tool runs. */
export interface PreToolEvent extends HookEventBase {
  readonly event: "pre_tool";
  /** Built-in or MCP tool name requested by the model. */
  readonly tool: string;
  /** Tool arguments with the same credential redaction as approval logs. */
  readonly detail: string;
  readonly outcome: null;
  readonly exit_code: null;
}

/**
 * Delivered after tool execution. `outcome` says whether dispatch succeeded;
 * it does not include the tool's returned text.
 */
export interface PostToolEvent extends HookEventBase {
  readonly event: "post_tool";
  readonly tool: string;
  readonly detail: string;
  readonly outcome: ToolOutcome;
  /** Reserved by the host payload. Current tool delivery leaves it null. */
  readonly exit_code: null;
}

interface ApprovalResolvedBase extends HookEventBase {
  readonly event: "approval_resolved";
  /** Approval action. For a judge-allowed command this is `run_command`. */
  readonly tool: string;
  /** Redacted action detail. It may still be truncated. */
  readonly detail: string;
  /** Reserved by the host payload. Current approval delivery leaves it null. */
  readonly exit_code: null;
  readonly extra: {
    readonly source: "approver" | "judge";
    readonly mode: ApprovalMode;
  } & Readonly<Record<string, string>>;
}

export interface ApproverResolvedEvent extends ApprovalResolvedBase {
  readonly outcome: ApproverOutcome;
  readonly extra: ApprovalResolvedBase["extra"] & {
    readonly source: "approver";
  };
}

/** Delivered only when the judge's final decision allows the command. */
export interface JudgeResolvedEvent extends ApprovalResolvedBase {
  readonly outcome: "allow";
  readonly extra: ApprovalResolvedBase["extra"] & {
    readonly source: "judge";
  };
}

export type ApprovalResolvedEvent = ApproverResolvedEvent | JudgeResolvedEvent;

export type HookEvent =
  | BeforeAgentEvent
  | AgentEndEvent
  | PreToolEvent
  | PostToolEvent
  | ApprovalResolvedEvent;

/** Configuration fields accepted by `[[hooks]]`, excluding execution itself. */
export interface HookConfig {
  /** Shown in denial output. Duplicate unnamed hooks are hard to diagnose. */
  readonly name?: string;
  readonly event: HookEventName;
  /** Operator-trusted shell command. It runs outside the model sandbox. */
  readonly command: string;
  /** Valid only for `before_agent` and `pre_tool`. Defaults to false. */
  readonly blocking?: boolean;
  /** Defaults to 10 seconds and is outside the model time budget. */
  readonly timeout_seconds?: number;
  /** Defaults to `deny`: a gate that fails open is not a gate. */
  readonly on_error?: HookFailurePolicy;
}

export function isHookEventName(value: string): value is HookEventName {
  return (HOOK_EVENTS as readonly string[]).includes(value);
}

export function canBlock(event: HookEventName): event is BlockingHookEventName {
  return (BLOCKING_HOOK_EVENTS as readonly string[]).includes(event);
}

/**
 * Read one hook event from the exact text supplied on stdin.
 *
 * The host writes one UTF-8 JSON object, not a JSON line stream. Trailing
 * whitespace is accepted; a second value is rejected. This function does not
 * repair truncated JSON or invent fields omitted by the host.
 */
export function parseHookEvent(input: string): HookEvent {
  const trimmed = input.trim();
  if (trimmed === "") {
    throw new HookInputError("hook stdin is empty");
  }
  let parsed: unknown;
  try {
    parsed = JSON.parse(trimmed);
  } catch (error) {
    throw new HookInputError("hook stdin is not one JSON value", error);
  }
  if (!isRecord(parsed)) {
    throw new HookInputError("hook event must be a JSON object");
  }
  const event = parsed.event;
  if (typeof event !== "string" || !isHookEventName(event)) {
    throw new HookInputError("hook event name is missing or unsupported");
  }
  assertNullableString(parsed, "session_id");
  assertNullableString(parsed, "workspace");
  const actionEvent =
    event === "pre_tool" || event === "post_tool" || event === "approval_resolved";
  assertNullableString(parsed, "tool", actionEvent);
  assertNullableString(parsed, "detail", actionEvent);
  if (parsed.exit_code !== null) {
    throw new HookInputError("current hook delivery leaves exit_code null");
  }
  assertOutcome(parsed, event);
  assertExtra(parsed, event);
  return parsed as unknown as HookEvent;
}

/**
 * Validate a hook definition before writing it to configuration.
 * This catches contracts rejected during host startup; it does not execute
 * the command or prove that the named binary exists.
 */
export function validateHookConfig(config: HookConfig): readonly string[] {
  const errors: string[] = [];
  if (!isHookEventName(config.event)) {
    errors.push("event must be one of the five delivered hook events");
  } else if (config.blocking === true && !canBlock(config.event)) {
    errors.push(`${config.event} happens after the action and cannot block`);
  }
  if (config.command.trim() === "") {
    errors.push("command must not be empty");
  }
  if (
    config.timeout_seconds !== undefined &&
    (!Number.isInteger(config.timeout_seconds) || config.timeout_seconds < 0)
  ) {
    errors.push("timeout_seconds must be a non-negative integer");
  }
  if (
    config.on_error !== undefined &&
    !(HOOK_FAILURE_POLICIES as readonly string[]).includes(config.on_error)
  ) {
    errors.push("on_error must be deny or ignore");
  }
  return errors;
}

export class HookInputError extends Error {
  readonly cause: unknown;

  constructor(message: string, cause?: unknown) {
    super(message);
    this.name = "HookInputError";
    this.cause = cause;
  }
}

function assertOutcome(value: Record<string, unknown>, event: HookEventName): void {
  const outcome = value.outcome;
  if (event === "before_agent" || event === "pre_tool") {
    if (outcome !== null) {
      throw new HookInputError(`${event} delivers outcome as null`);
    }
    return;
  }
  if (typeof outcome !== "string") {
    throw new HookInputError(`${event} requires a string outcome`);
  }
  const allowed =
    event === "agent_end"
      ? AGENT_END_OUTCOMES
      : event === "post_tool"
        ? TOOL_OUTCOMES
        : [...APPROVER_OUTCOMES, "allow"];
  if (!(allowed as readonly string[]).includes(outcome)) {
    throw new HookInputError(`${event} outcome ${outcome} is not delivered by the host`);
  }
}

function assertExtra(value: Record<string, unknown>, event: HookEventName): void {
  const extra = value.extra;
  if (extra === undefined) {
    if (event === "approval_resolved") {
      throw new HookInputError("approval_resolved requires extra.source and extra.mode");
    }
    return;
  }
  if (!isRecord(extra)) {
    throw new HookInputError("extra must be an object of strings");
  }
  for (const [key, entry] of Object.entries(extra)) {
    if (typeof entry !== "string") {
      throw new HookInputError(`extra.${key} must be a string`);
    }
  }
  if (event !== "approval_resolved") return;
  if (extra.source !== "approver" && extra.source !== "judge") {
    throw new HookInputError("approval_resolved extra.source must be approver or judge");
  }
  if (typeof extra.mode !== "string" || !(APPROVAL_MODES as readonly string[]).includes(extra.mode)) {
    throw new HookInputError("approval_resolved extra.mode is missing or unsupported");
  }
  const outcome = value.outcome;
  if (extra.source === "judge" && outcome !== "allow") {
    throw new HookInputError("judge approval events are delivered only for allow");
  }
  if (extra.source === "approver" && outcome === "allow") {
    throw new HookInputError("approver outcomes are allow_once, always_allow, or deny");
  }
}

function assertNullableString(
  value: Record<string, unknown>,
  field: string,
  required = false,
): void {
  const entry = value[field];
  if (typeof entry === "string" || (!required && entry === null)) return;
  throw new HookInputError(
    required ? `${field} must be a string` : `${field} must be a string or null`,
  );
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}
