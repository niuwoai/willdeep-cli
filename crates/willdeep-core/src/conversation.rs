//! Human-facing projection. Never mutate model history or use display progress
//! as evidence that a task has actually completed.
use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use crate::types::{Message, MessageSource, Role};

/// 一次工具调用在回放里的样子：名字、脱敏摘要、有没有结果。原始参数不出投影——
/// Web 端的会话详情是公共投影，路径与凭据不能跟着走。
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ToolTrace {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// 历史里有没有这条调用的结果；没有说明轮次在这里被打断了。
    pub completed: bool,
}

/// 工具调用的一句话摘要：够回答「执行了什么」，又不把路径和凭据带出去。
///
/// - 命令保留词序，看起来像路径的词换成 `…`，包括 `/abs`、`~/x` 与 `--flag=/abs`；
/// - 按命令审批同一套规则打码，再按**字符**截断——用户完全可能把 token 粘进
///   命令行。不能只留前几个词，否则 `for u in …` 这种循环看起来像半截命令。
///
/// 认不出的工具返回 `None`：宁可不显示，也不要把一段不知道含什么的参数漏出去。
pub fn tool_detail(name: &str, arguments: &str) -> Option<String> {
    const MAX_DETAIL_CHARS: usize = 96;
    let parsed = serde_json::from_str::<serde_json::Value>(arguments).ok()?;
    let detail = match name {
        "run_command" => {
            let command = parsed.get("command")?.as_str()?;
            command
                .split_whitespace()
                .map(elide_path_like)
                .collect::<Vec<_>>()
                .join(" ")
        }
        "spawn_agent" => {
            let profile = parsed
                .get("profile")
                .and_then(|value| value.as_str())
                .unwrap_or("generalist");
            match parsed.get("label").and_then(|value| value.as_str()) {
                Some(label) if !label.trim().is_empty() => {
                    format!("{profile} · {}", label.trim())
                }
                _ => profile.to_owned(),
            }
        }
        "ask_user" => {
            // 问题本身就是给用户看的；记进工具行，对话框失效后还找得回来。
            let question = parsed.get("question")?.as_str()?.trim();
            if question.is_empty() {
                return None;
            }
            question.split_whitespace().collect::<Vec<_>>().join(" ")
        }
        _ => return None,
    };
    let redacted = crate::judge::redact_credentials(&detail);
    if redacted.trim().is_empty() {
        return None;
    }
    let mut chars = redacted.chars();
    let excerpt: String = chars.by_ref().take(MAX_DETAIL_CHARS).collect();
    Some(if chars.next().is_some() {
        format!("{excerpt}…")
    } else {
        excerpt
    })
}

/// 看起来像路径的词换成省略号。
///
/// 判定放宽到 `--flag=/abs` 这种形式：把路径藏在等号后面是最常见的漏法。
fn elide_path_like(token: &str) -> String {
    let candidate = token.split_once('=').map_or(token, |(_, value)| value);
    if candidate.starts_with('/') || candidate.starts_with('~') || candidate.starts_with("./") {
        return match token.split_once('=') {
            Some((flag, _)) => format!("{flag}=…"),
            None => "…".to_owned(),
        };
    }
    token.to_owned()
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StepStatus {
    Pending,
    InProgress,
    Done,
    Skipped,
    Failed,
    Blocked,
}

impl StepStatus {
    pub fn token(&self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::InProgress => "in_progress",
            Self::Done => "done",
            Self::Skipped => "skipped",
            Self::Failed => "failed",
            Self::Blocked => "blocked",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanStep {
    pub id: String,
    pub text: String,
    pub status: StepStatus,
    #[serde(default)]
    pub detail: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub depends_on: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Plan {
    #[serde(default)]
    pub summary: String,
    pub steps: Vec<PlanStep>,
}

#[derive(Clone, Debug, Serialize)]
pub struct ConversationItem {
    pub role: &'static str,
    pub content: String,
    pub attachment_count: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attachment_reference: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plan: Option<Plan>,
    pub details: Vec<String>,
    /// 这条 assistant 消息发起的工具调用，按发起顺序；回放时逐条落成工具行。
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<ToolTrace>,
}

/// Both surfaces use exactly this projection when reopening or refreshing a
/// session. Only explicitly sourced host messages are reclassified.
pub fn project(messages: &[Message], current_plan: Option<&Plan>) -> Vec<ConversationItem> {
    let mut items: Vec<ConversationItem> = Vec::new();
    let mut active: Option<usize> = None;
    let plan_calls: HashSet<&str> = messages
        .iter()
        .filter(|message| message.role == Role::Assistant)
        .flat_map(|message| &message.tool_calls)
        .filter(|call| call.name == "update_plan")
        .map(|call| call.id.as_str())
        .collect();
    // 哪些工具调用在历史里有结果：没有的那些，轮次就是在那里被打断的。
    let answered: HashSet<&str> = messages
        .iter()
        .filter(|message| message.role == Role::Tool)
        .filter_map(|message| message.tool_call_id.as_deref())
        .collect();
    for message in messages {
        if message.role == Role::Tool
            && message
                .tool_call_id
                .as_deref()
                .is_some_and(|id| plan_calls.contains(id))
            && let Some(receipt) = crate::planning::receipt(&message.content)
        {
            let index = if !receipt.replaced
                && let Some(index) = active
            {
                index
            } else {
                items.push(item("plan", String::new(), 0));
                items.len() - 1
            };
            items[index].plan = Some(receipt.plan);
            active = Some(index);
            continue;
        }
        let role = match message.role {
            Role::User if message.source == Some(MessageSource::HostInstruction) => "system",
            Role::User => "user",
            // 只带工具调用、正文为空的 assistant 消息也要进投影：工具行回放靠它。
            Role::Assistant
                if !message.content.trim().is_empty() || !message.tool_calls.is_empty() =>
            {
                "assistant"
            }
            _ => continue,
        };
        // The goal engine's completion tag is protocol, not prose: history keeps
        // it, the projection drops it before anything renders or parses.
        let content = if role == "assistant" {
            visible_assistant_text(&message.content)
        } else {
            message.content.as_str()
        };
        if role == "assistant" {
            let existing = active.and_then(|index| items[index].plan.as_ref());
            if let Some((plan, is_new)) = parse_plan_reply(content, existing) {
                let index = if !is_new && let Some(index) = active {
                    index
                } else {
                    items.push(item("plan", String::new(), 0));
                    items.len() - 1
                };
                items[index].plan = Some(plan);
                if !items[index].details.iter().any(|detail| detail == content) {
                    items[index].details.push(content.to_owned());
                }
                active = Some(index);
                continue;
            }
        }
        let mut entry = item(role, content.to_owned(), message.attachments.len());
        entry.attachment_reference = attachment_reference(&message.attachments);
        if role == "user" {
            entry.content = visible_user_text(&message.content).to_owned();
            // A context-only turn has no operator bubble to display.
            if entry.content.trim().is_empty() && entry.attachment_count == 0 {
                continue;
            }
        }
        if role == "system" {
            entry.details.push(std::mem::take(&mut entry.content));
        }
        if role == "assistant" {
            // A reply that was nothing but the completion tag leaves no bubble;
            // tool-only messages still project their calls.
            if entry.content.trim().is_empty() && message.tool_calls.is_empty() {
                continue;
            }
            entry.tools = message
                .tool_calls
                .iter()
                .map(|call| ToolTrace {
                    name: call.name.clone(),
                    detail: tool_detail(&call.name, &call.arguments),
                    completed: answered.contains(call.id.as_str()),
                })
                .collect();
        }
        items.push(entry);
    }
    // Persisted host state wins over model self-reports. Do not apply a newer
    // phase's state to an unrelated historical plan.
    if let Some(plan) = current_plan.filter(|plan| !plan.steps.is_empty()) {
        let matching = items.iter().rposition(|item| {
            item.plan.as_ref().is_some_and(|p| {
                p.steps.len() == plan.steps.len()
                    && (p
                        .steps
                        .iter()
                        .zip(&plan.steps)
                        .all(|(a, b)| a.text == b.text)
                        || p.steps.iter().all(|step| {
                            step.text == step.id
                                && !step.id.chars().all(|c| c.is_ascii_digit())
                                && step_index(plan, &step.id).is_some()
                        }))
            })
        });
        if let Some(index) = matching {
            items[index].plan = Some(plan.clone());
        } else {
            let mut entry = item("plan", String::new(), 0);
            entry.plan = Some(plan.clone());
            items.push(entry);
        }
    }
    items
}

use willdeep_runtime_protocol::{
    ATTACHED_CONTEXT_HEADER, USER_MESSAGE_BOUNDARY as USER_TEXT_BOUNDARY,
};

/// The desktop composer escapes this boundary inside attachments. Require the
/// exact protocol header and a standalone boundary, never guess from skill names
/// or an ordinary sentence that happens to mention the protocol.
pub fn user_authored_text(content: &str) -> Option<&str> {
    let normalized = content.trim_start();
    let (header, _) = normalized.split_once('\n')?;
    if header.trim_end_matches('\r') != ATTACHED_CONTEXT_HEADER {
        return None;
    }
    let mut offset = 0;
    for line in normalized.split_inclusive('\n') {
        offset += line.len();
        if line.trim_end_matches(['\r', '\n']) == USER_TEXT_BOUNDARY {
            return Some(normalized[offset..].trim());
        }
    }
    None
}

/// Display-only projection. Persisted prompts and model context remain intact.
/// Recognize complete host routing envelopes at the end, never arbitrary XML
/// snippets, inline examples, incomplete blocks, or fenced user quotations.
pub fn visible_user_text(content: &str) -> &str {
    let text = user_authored_text(content).unwrap_or(content);
    // `/goal` 的持久目标包装是给模型看的，聊天区只留用户自己写的那句。
    // 只认 TUI 拼出来的完整块，半截或代码围栏里引用的一律原样保留。
    let text = strip_goal_envelope(text);
    let Some((authored, envelope)) = text.rsplit_once("\n\n<runtime-route ") else {
        return text;
    };
    if authored.matches("```").count() % 2 != 0 {
        return text;
    }
    let Some((attributes, body)) = envelope.split_once(">\n") else {
        return text;
    };
    if !attributes.starts_with("tier=\"")
        || !attributes.contains("\" profile=\"")
        || !attributes.contains("\" confidence=\"")
        || !attributes.ends_with('"')
    {
        return text;
    }
    let bounded = body.starts_with("The runtime classified this as bounded `")
        && body.ends_with(" work. Compile a task packet, keep read_files separate from write_files, and delegate it before doing the implementation in the parent. Give a verifier whenever completion is command-decidable.\n</runtime-route>");
    let worker = attributes.starts_with("tier=\"worker\"")
        && body.starts_with("The runtime dispatched this bounded read-only preflight before the standard model. Use the report as evidence, verify citations when needed, and answer without re-reading the same material unless it is incomplete.\n<worker-report>\n")
        && body.ends_with("\n</worker-report>\n</runtime-route>");
    if bounded || worker { authored } else { text }
}

/// Display-only projection for goal-mode replies. The model opens its final
/// reply with a status tag (see `GOAL_COMPLETE_MARKER`) so the engine can tell
/// the goal was met; the tag is protocol, the rest is prose for the user.
/// Stored messages keep the tag for the engine, feedback signals and audits —
/// only the projection drops it. Tags elsewhere in the text stay untouched: the
/// engine only honours a leading one, so anything else is the model's own words.
pub fn visible_assistant_text(content: &str) -> &str {
    let trimmed = content.trim_start();
    let Some(rest) = trimmed.strip_prefix("<goal-status>") else {
        return content;
    };
    let Some((_, after)) = rest.split_once("</goal-status>") else {
        return content;
    };
    after.trim_start()
}

/// 去掉 `/goal` 提交时拼在前面的目标块，返回用户写的那句。
///
/// 块必须从头开始、单独成段，正文也不能落在未闭合的代码围栏里，
/// 否则用户自己贴的一段 `<goal>` 示例会被误删。
fn strip_goal_envelope(text: &str) -> &str {
    let rest = text
        .strip_prefix("<goal>\n")
        .and_then(|rest| {
            rest.split_once("\n</goal>\nContinue until this goal is genuinely complete.\n\n")
        })
        .map(|(_, authored)| authored);
    match rest {
        Some(authored) if authored.matches("```").count() % 2 == 0 => authored,
        _ => text,
    }
}

fn item(role: &'static str, content: String, attachment_count: usize) -> ConversationItem {
    ConversationItem {
        role,
        content,
        attachment_count,
        attachment_reference: None,
        plan: None,
        details: Vec::new(),
        tools: Vec::new(),
    }
}

/// Content-addressed references cannot silently select another message after compression or rewind.
pub fn attachment_reference(attachments: &[crate::types::MessageAttachment]) -> Option<String> {
    use sha2::{Digest, Sha256};
    if attachments.is_empty() {
        return None;
    }
    struct HashWriter(Sha256);
    impl std::io::Write for HashWriter {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.update(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut writer = HashWriter(Sha256::new());
    serde_json::to_writer(&mut writer, attachments).ok()?;
    Some(format!("{:x}", writer.0.finalize()))
}

/// Parse only complete, explicitly typed fences. Ordinary prose, user-pasted
/// examples, unclosed streams and unknown statuses are never consumed.
pub fn parse_plan_reply(content: &str, existing: Option<&Plan>) -> Option<(Plan, bool)> {
    let mut plan = existing.cloned();
    let mut new_plan = false;
    let mut found = false;
    let mut lines = content.lines();
    while let Some(line) = lines.next() {
        let trimmed = line.trim();
        if !trimmed.starts_with("```") {
            continue;
        }
        let kind = trimmed.strip_prefix("```")?;
        if kind != "plan" && kind != "progress" {
            return None;
        }
        let mut body = Vec::new();
        let mut closed = false;
        for line in lines.by_ref() {
            if line.trim() == "```" {
                closed = true;
                break;
            }
            if !line.trim().is_empty() {
                body.push(line.trim());
            }
        }
        if !closed || body.is_empty() {
            return None;
        }
        if kind == "plan" {
            let steps = body
                .iter()
                .enumerate()
                .map(|(index, line)| {
                    let (number, text) = line.split_once(". ")?;
                    if number.parse::<usize>().ok()? != index + 1 || text.trim().is_empty() {
                        return None;
                    }
                    Some(PlanStep {
                        id: number.to_owned(),
                        text: text.to_owned(),
                        status: StepStatus::Pending,
                        detail: None,
                        depends_on: Vec::new(),
                    })
                })
                .collect::<Option<Vec<_>>>()?;
            plan = Some(Plan {
                summary: String::new(),
                steps,
            });
            new_plan = true;
        } else {
            let updates = body
                .iter()
                .map(|line| parse_update(line))
                .collect::<Option<Vec<_>>>()?;
            if let Some(plan) = plan.as_mut() {
                let indices = updates
                    .iter()
                    .map(|(key, _)| step_index(plan, key))
                    .collect::<Option<Vec<_>>>()?;
                for ((_, status), index) in updates.into_iter().zip(indices) {
                    plan.steps[index].status = status;
                }
            } else {
                plan = Some(Plan {
                    summary: String::new(),
                    steps: updates
                        .into_iter()
                        .map(|(id, status)| PlanStep {
                            text: id.clone(),
                            id,
                            status,
                            detail: None,
                            depends_on: Vec::new(),
                        })
                        .collect(),
                });
            }
        }
        found = true;
    }
    found.then_some((plan?, new_plan))
}

fn parse_update(line: &str) -> Option<(String, StepStatus)> {
    let (id, status) = line.split_once(':').or_else(|| line.split_once(". "))?;
    let id = id.trim();
    if id.is_empty()
        || !id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return None;
    }
    let status =
        serde_json::from_value(serde_json::Value::String(status.trim().to_owned())).ok()?;
    Some((id.to_owned(), status))
}

fn step_index(plan: &Plan, key: &str) -> Option<usize> {
    plan.steps
        .iter()
        .position(|step| step.id == key)
        .or_else(|| {
            key.parse::<usize>()
                .ok()
                .filter(|index| *index > 0 && *index <= plan.steps.len())
                .map(|index| index - 1)
        })
        .or_else(|| {
            plan.steps.iter().position(|step| {
                step.text
                    .split(|c: char| !c.is_ascii_alphanumeric() && c != '-' && c != '_')
                    .any(|word| word == key)
            })
        })
}

#[cfg(test)]
mod tests;
