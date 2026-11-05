use super::*;
use serde_json::{Value, json};
use willdeep_runtime_protocol::child_session::{
    CHILD_SESSION_DEPENDENCY_LIMIT, StartChildSessionParams,
};

pub(super) struct Request {
    pub operation: &'static str,
    pub params: Value,
    pub request_id: Option<uuid::Uuid>,
    pub needs_confirmation: bool,
}

fn argument<'a>(tail: &mut &'a str, language: Language) -> Result<&'a str> {
    let (value, rest) = word(tail);
    anyhow::ensure!(
        !value.is_empty(),
        "{}",
        language.text(
            "选项缺少参数",
            "Missing option value",
            "オプションの値がありません"
        )
    );
    *tail = rest;
    Ok(value)
}

fn id(value: &str, language: Language) -> Result<uuid::Uuid> {
    value
        .parse()
        .context(language.text("无效的 UUID", "Invalid UUID", "UUID が無効です"))
}

fn revision(value: &str, language: Language) -> Result<u64> {
    let value = value.parse::<u64>().context(language.text(
        "无效的版本号",
        "Invalid revision",
        "バージョンが無効です",
    ))?;
    anyhow::ensure!(
        value > 0,
        "{}",
        language.text(
            "版本号必须大于零",
            "Revision must be positive",
            "バージョンは正の値が必要です"
        )
    );
    Ok(value)
}

fn subscription(value: &str, language: Language) -> Result<&'static str> {
    match value {
        "none" => Ok("none"),
        "inbox" => Ok("inbox"),
        "auto_wake" | "auto-wake" => Ok("auto_wake"),
        _ => bail!(
            "{}",
            language.text(
                "通知策略须为 none、inbox 或 auto_wake",
                "Subscription must be none, inbox or auto_wake",
                "通知設定は none、inbox、auto_wake のいずれかです"
            )
        ),
    }
}

fn assignment_options<'a>(
    mut tail: &'a str,
    start: bool,
    params: &mut Value,
    language: Language,
) -> Result<(&'a str, Option<uuid::Uuid>)> {
    let mut dependencies = Vec::new();
    let mut request_id = None;
    let mut seen = HashSet::new();
    loop {
        let (flag, remaining) = word(tail);
        if !flag.starts_with("--") {
            break;
        }
        tail = remaining;
        if flag != "--depends-on" {
            anyhow::ensure!(
                seen.insert(flag),
                "{}: {flag}",
                language.text(
                    "选项不能重复",
                    "Duplicate option",
                    "オプションを重複指定できません"
                )
            );
        }
        match flag {
            "--subscription" => {
                params["subscription"] =
                    subscription(argument(&mut tail, language)?, language)?.into()
            }
            "--depends-on" => dependencies.push(id(argument(&mut tail, language)?, language)?),
            "--request-id" => request_id = Some(id(argument(&mut tail, language)?, language)?),
            "--fork" if start => {
                params["context"] = "fork".into();
                params["through_turn_id"] = json!(id(argument(&mut tail, language)?, language)?);
            }
            "--worktree" if start => params["workspace_mode"] = "worktree".into(),
            "--snapshot" if start => params["worktree_base"] = "snapshot".into(),
            "--model" if start => params["model"] = argument(&mut tail, language)?.into(),
            _ => bail!(
                "{}: {flag}",
                language.text("未知选项", "Unknown option", "不明なオプション")
            ),
        }
    }
    anyhow::ensure!(
        dependencies.len() <= CHILD_SESSION_DEPENDENCY_LIMIT,
        "{} ({CHILD_SESSION_DEPENDENCY_LIMIT})",
        language.text(
            "前置任务数量超过上限",
            "Too many prerequisites",
            "前提タスクが上限を超えています"
        )
    );
    let unique: HashSet<_> = dependencies.iter().collect();
    anyhow::ensure!(
        unique.len() == dependencies.len(),
        "{}",
        language.text(
            "前置任务不能重复",
            "Prerequisites must be unique",
            "前提タスクを重複指定できません"
        )
    );
    if !dependencies.is_empty() {
        params["depends_on"] = json!(dependencies);
    }
    Ok((tail, request_id))
}

pub(super) fn parse(
    arguments: &str,
    parent: uuid::Uuid,
    language: Language,
) -> Result<Option<Request>> {
    let (action, rest) = word(arguments);
    let mut request = Request {
        operation: "",
        params: json!({"parent_session_id":parent}),
        request_id: None,
        needs_confirmation: false,
    };
    match action {
        "start" => {
            request.operation = "session.child.start";
            let (tail, request_id) = assignment_options(rest, true, &mut request.params, language)?;
            let Some((title, prompt)) = tail.split_once('|') else {
                return Ok(None);
            };
            if title.trim().is_empty() || prompt.trim().is_empty() {
                return Ok(None);
            }
            request.params["title"] = title.trim().into();
            request.params["prompt"] = prompt.trim().into();
            request.request_id = request_id;
            let typed: StartChildSessionParams = serde_json::from_value(request.params.clone())?;
            typed.validate().map_err(anyhow::Error::msg)?;
        }
        "continue" => {
            request.operation = "session.child.continue";
            let (child, tail) = word(rest);
            request.params["child_session_id"] = json!(id(child, language)?);
            let (prompt, request_id) =
                assignment_options(tail, false, &mut request.params, language)?;
            if prompt.is_empty() {
                return Ok(None);
            }
            request.params["prompt"] = prompt.into();
            request.request_id = request_id;
        }
        "list" => {
            request.operation = "session.child.task.list";
            request.params["limit"] = 50.into();
            if !rest.is_empty() {
                request.params["after"] = json!(id(rest, language)?);
            }
        }
        "show" | "stop" => {
            request.operation = if action == "show" {
                "session.child.task.get"
            } else {
                "session.child.task.stop"
            };
            request.params["assignment_id"] = json!(id(rest, language)?);
        }
        "retry" => {
            request.operation = "session.child.retry";
            let (assignment, value) = word(rest);
            request.params["assignment_id"] = json!(id(assignment, language)?);
            request.params["expected_result_revision"] = json!(revision(value, language)?);
        }
        "link" => {
            request.operation = "session.child.link.get";
            request.params["child_session_id"] = json!(id(rest, language)?);
        }
        "takeover" | "detach" => {
            request.operation = if action == "takeover" {
                "session.child.takeover"
            } else {
                "session.child.detach"
            };
            let (child, mut tail) = word(rest);
            request.params["child_session_id"] = json!(id(child, language)?);
            let mut seen = HashSet::new();
            let mut confirmed = false;
            while !tail.is_empty() {
                let flag = argument(&mut tail, language)?;
                anyhow::ensure!(
                    seen.insert(flag),
                    "{}: {flag}",
                    language.text(
                        "选项不能重复",
                        "Duplicate option",
                        "オプションを重複指定できません"
                    )
                );
                match flag {
                    "--expected-control-revision" => {
                        request.params["expected_control_revision"] =
                            json!(revision(argument(&mut tail, language)?, language)?)
                    }
                    "--request-id" => {
                        request.request_id = Some(id(argument(&mut tail, language)?, language)?)
                    }
                    "--yes" => confirmed = true,
                    _ => bail!(
                        "{}: {flag}",
                        language.text("未知选项", "Unknown option", "不明なオプション")
                    ),
                }
            }
            anyhow::ensure!(
                request.params.get("expected_control_revision").is_some(),
                "{}",
                language.text(
                    "请先用 link 查询，再提供 --expected-control-revision",
                    "Query link first, then supply --expected-control-revision",
                    "先に link で確認し、--expected-control-revision を指定してください"
                )
            );
            request.needs_confirmation = !confirmed;
        }
        "subscription" => {
            request.operation = "session.child.task.subscription.set";
            let (verb, tail) = word(rest);
            if verb != "set" {
                return Ok(None);
            }
            let (assignment, mut tail) = word(tail);
            request.params["assignment_id"] = json!(id(assignment, language)?);
            let mut seen = HashSet::new();
            while !tail.is_empty() {
                let flag = argument(&mut tail, language)?;
                anyhow::ensure!(
                    seen.insert(flag),
                    "{}: {flag}",
                    language.text(
                        "选项不能重复",
                        "Duplicate option",
                        "オプションを重複指定できません"
                    )
                );
                match flag {
                    "--expected-subscription-revision" => {
                        request.params["expected_subscription_revision"] =
                            json!(revision(argument(&mut tail, language)?, language)?)
                    }
                    "--subscription" => {
                        request.params["subscription"] =
                            subscription(argument(&mut tail, language)?, language)?.into()
                    }
                    "--request-id" => {
                        request.request_id = Some(id(argument(&mut tail, language)?, language)?)
                    }
                    _ => bail!(
                        "{}: {flag}",
                        language.text("未知选项", "Unknown option", "不明なオプション")
                    ),
                }
            }
            anyhow::ensure!(
                request
                    .params
                    .get("expected_subscription_revision")
                    .is_some()
                    && request.params.get("subscription").is_some(),
                "{}",
                language.text(
                    "须提供 --expected-subscription-revision 与 --subscription",
                    "Supply --expected-subscription-revision and --subscription",
                    "--expected-subscription-revision と --subscription を指定してください"
                )
            );
        }
        _ => return Ok(None),
    }
    Ok(Some(request))
}

#[cfg(test)]
mod tests;
