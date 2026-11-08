use super::*;
use willdeep_core::conversation::{Plan, StepStatus};

const PLAN_PREFIX: &str = "Plan: ";

#[derive(serde::Serialize, serde::Deserialize)]
pub(super) struct PlanTranscript {
    pub plan: Plan,
    pub details: Vec<String>,
    pub expanded: bool,
}

fn plan_entry(card: &PlanTranscript) -> String {
    format!(
        "{PLAN_PREFIX}{}",
        serde_json::to_string(card).expect("plan serializes")
    )
}

pub(super) fn decode_plan_entry(entry: &str) -> Option<PlanTranscript> {
    serde_json::from_str(entry.strip_prefix(PLAN_PREFIX)?).ok()
}

pub(super) fn render_plan(plan: &Plan, width: usize, language: Language) -> Vec<Line<'static>> {
    let ended = plan
        .steps
        .iter()
        .filter(|step| matches!(step.status, StepStatus::Done | StepStatus::Skipped))
        .count();
    let running = plan
        .steps
        .iter()
        .filter(|step| step.status == StepStatus::InProgress)
        .count();
    let running_label = language.text("进行中", "running", "実行中");
    let mut lines = vec![Line::styled(
        format!(
            "┌ ▣ {ended}/{} · {running} {running_label} · /plan",
            plan.steps.len()
        ),
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
    )];
    if !plan.summary.is_empty() {
        lines.extend(render_assistant_markdown(
            &plan.summary,
            width.saturating_sub(2),
        ));
    }
    for step in &plan.steps {
        let (symbol, color) = match step.status {
            StepStatus::Pending => ("○", Color::DarkGray),
            StepStatus::InProgress => ("›", Color::Cyan),
            StepStatus::Done => ("✓", Color::Green),
            StepStatus::Skipped => ("–", Color::DarkGray),
            StepStatus::Failed => ("!", Color::Red),
            StepStatus::Blocked => ("⊘", Color::Yellow),
        };
        lines.push(Line::from(vec![
            Span::styled(format!("│ {symbol} "), Style::default().fg(color)),
            Span::raw(step.text.clone()),
        ]));
        if let Some(detail) = &step.detail {
            lines.push(Line::styled(
                format!("│   {detail}"),
                Style::default().fg(Color::DarkGray),
            ));
        }
        if step.status == StepStatus::Pending {
            let waiting = willdeep_core::planning::waiting_for(plan, step);
            if !waiting.is_empty() {
                let titles = waiting
                    .iter()
                    .map(|id| {
                        plan.steps
                            .iter()
                            .find(|other| &other.id == id)
                            .map_or(id.as_str(), |other| other.text.as_str())
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                lines.push(Line::styled(
                    format!(
                        "│   {}: {titles}",
                        language.text("等待", "Waiting for", "待機中")
                    ),
                    Style::default().fg(Color::DarkGray),
                ));
            }
        }
    }
    lines.push(Line::styled("└", Style::default().fg(Color::Cyan)));
    lines
}

pub(super) fn append_plan_result(entries: &mut Vec<String>, output: &str) -> bool {
    let Some(receipt) = willdeep_core::planning::receipt(output) else {
        return false;
    };
    let previous = entries
        .iter()
        .rposition(|entry| decode_plan_entry(entry).is_some());
    if let Some(index) = previous {
        let mut card = decode_plan_entry(&entries[index]).expect("plan entry");
        let same_plan = card.plan.has_same_steps(&receipt.plan);
        if receipt.replaced || !same_plan {
            card.details.clear();
        }
        card.plan = receipt.plan;
        entries[index] = plan_entry(&card);
    } else {
        entries.push(plan_entry(&PlanTranscript {
            plan: receipt.plan,
            details: Vec::new(),
            expanded: false,
        }));
    }
    true
}

/// Replace a plan in place for live replies, using the same parser as history.
pub(super) fn append_plan_reply(entries: &mut Vec<String>, reply: &str) -> bool {
    let latest = entries
        .iter()
        .rposition(|entry| decode_plan_entry(entry).is_some());
    let previous = latest.and_then(|index| decode_plan_entry(&entries[index]));
    let Some((plan, is_new)) = willdeep_core::conversation::parse_plan_reply(
        reply,
        previous.as_ref().map(|card| &card.plan),
    ) else {
        return false;
    };
    let mut card = previous.unwrap_or_else(|| PlanTranscript {
        plan: plan.clone(),
        details: Vec::new(),
        expanded: false,
    });
    if is_new {
        card.details.clear();
    }
    card.plan = plan;
    if !card.details.iter().any(|detail| detail == reply) {
        card.details.push(reply.to_owned());
    }
    if let Some(index) = latest {
        entries[index] = plan_entry(&card);
    } else {
        entries.push(plan_entry(&card));
    }
    true
}

#[cfg(test)]
pub(super) fn projected_transcript(
    messages: &[Message],
    plan: Option<&Plan>,
    language: Language,
) -> Vec<String> {
    project_session(messages, plan, language).0
}

/// 聊天记录和与之逐行对齐的附件。附件只挂在用户那一行上，其余行是空的。
///
/// 用户行不再把附件塞进 `[N attachment(s)]` 这种后缀：实时回显的是纯 `You: 原文`，
/// 重开后必须长得一样，附件正文改由画的时候从对齐的槽里取。
pub(super) fn project_session(
    messages: &[Message],
    plan: Option<&Plan>,
    language: Language,
) -> (Vec<String>, Vec<Vec<MessageAttachment>>) {
    let mut rows = Vec::new();
    let mut attachments = Vec::new();
    // 投影会跳过一部分消息，所以不能按下标对齐，得跟着投影的顺序走。
    let mut user_attachments = user_attachment_slots(messages);
    for item in willdeep_core::conversation::project(messages, plan) {
        let is_user = item.role == "user" && item.workflow.is_none() && item.plan.is_none();
        let produced = if let Some(workflow) = item.workflow {
            vec![workflow_ui::entry(workflow, false)]
        } else if let Some(plan) = item.plan {
            vec![plan_entry(&PlanTranscript {
                plan,
                details: item.details,
                expanded: false,
            })]
        } else {
            match item.role {
                "user" => vec![format!("You: {}", item.content)],
                "system" => vec![format!(
                    "System: {}",
                    language.text(
                        "系统自动推进",
                        "Automatic system activity",
                        "システムの自動進行"
                    )
                )],
                _ => {
                    // 与实时路径同一套行：先正文（有的话），再逐条工具行。
                    let mut rows = Vec::with_capacity(1 + item.tools.len());
                    if !item.content.trim().is_empty() {
                        rows.push(format!("WillDeep: {}", item.content));
                    }
                    rows.extend(item.tools.iter().map(narration::replayed_tool_row));
                    rows
                }
            }
        };
        // 一条用户消息投影成一行，附件挂在这一行；它展开出的其余行没有附件。
        // 对不上时给空槽而不是崩：重开历史不值得为一条附件对不齐就打不开。
        let slot = if is_user && !user_attachments.is_empty() {
            user_attachments.remove(0)
        } else {
            Vec::new()
        };
        for (index, row) in produced.into_iter().enumerate() {
            rows.push(row);
            attachments.push(if index == 0 { slot.clone() } else { Vec::new() });
        }
    }
    (rows, attachments)
}

/// 按投影会保留的顺序，取出每条用户消息的附件。跳过规则与 `conversation::project`
/// 一致：宿主指令算系统消息，没有可见正文也没有附件的用户消息不出现。
fn user_attachment_slots(messages: &[Message]) -> Vec<Vec<MessageAttachment>> {
    messages
        .iter()
        .filter(|message| message.role == willdeep_core::Role::User)
        .filter(|message| {
            message.source != Some(willdeep_core::types::MessageSource::HostInstruction)
        })
        .filter(|message| {
            !willdeep_core::conversation::visible_user_text(&message.content)
                .trim()
                .is_empty()
                || !message.attachments.is_empty()
        })
        .map(|message| message.attachments.clone())
        .collect()
}

pub(super) fn toggle_plan_details(entries: &mut [String]) {
    if let Some(entry) = entries
        .iter_mut()
        .rev()
        .find(|entry| decode_plan_entry(entry).is_some())
    {
        let mut card = decode_plan_entry(entry).expect("selected plan");
        card.expanded = !card.expanded;
        *entry = plan_entry(&card);
    }
}

pub(super) fn sync_persisted_plan(entries: &mut Vec<String>, plan: &Plan) {
    let latest = entries
        .iter()
        .rposition(|entry| decode_plan_entry(entry).is_some());
    if let Some(index) = latest {
        let mut card = decode_plan_entry(&entries[index]).expect("plan entry");
        let same_plan = card.plan.has_same_steps(plan);
        if !same_plan {
            card.details.clear();
        }
        card.plan = plan.clone();
        entries[index] = plan_entry(&card);
        return;
    }
    // The TUI can attach mid-turn, after the plan event was emitted, or the
    // event can arrive while the session is not yet marked runtime-managed.
    // Keep one visible card for the current plan and append it only when none exists.
    entries.push(plan_entry(&PlanTranscript {
        plan: plan.clone(),
        details: Vec::new(),
        expanded: false,
    }));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn structured_updates_show_multiple_running_steps_and_dependency_waits() {
        let plan: Plan = serde_json::from_value(serde_json::json!({"summary":"", "steps":[
            {"id":"ci", "text":"Release build", "status":"in_progress"},
            {"id":"install", "text":"Local install", "status":"in_progress"},
            {"id":"brew", "text":"Update formula", "status":"pending", "depends_on":["ci"]}
        ]}))
        .unwrap();
        let mut entries = Vec::new();
        let receipt = willdeep_core::planning::Receipt::new(plan.clone(), true);
        assert!(append_plan_result(
            &mut entries,
            &serde_json::to_string(&receipt).unwrap()
        ));
        let text = Text::from(render_plan(&plan, 80, Language::ZhCn)).to_string();
        assert!(text.contains("2 进行中"));
        assert!(text.contains("› Release build"));
        assert!(text.contains("› Local install"));
        assert!(text.contains("等待: Release build"));
        let mut next = plan;
        next.steps[0].status = StepStatus::Done;
        let receipt = willdeep_core::planning::Receipt::new(next, false);
        assert!(append_plan_result(
            &mut entries,
            &serde_json::to_string(&receipt).unwrap()
        ));
        assert_eq!(entries.len(), 1);
        assert_eq!(
            decode_plan_entry(&entries[0]).unwrap().plan.steps[1].status,
            StepStatus::InProgress
        );
    }

    const PLAN: &str = "```plan\n1. 验证接口\n2. 发布\n```";
    const PROGRESS: &str = "没有新增状态变化。\n```progress\n1: done\n2: skipped\n```";

    #[test]
    fn persisted_plan_without_a_card_is_appended_once() {
        let plan: Plan = serde_json::from_value(serde_json::json!({"summary":"", "steps":[
            {"id":"a", "text":"检查", "status":"in_progress"},
            {"id":"b", "text":"发布", "status":"pending", "depends_on":["a"]}
        ]}))
        .unwrap();
        let mut entries = Vec::new();
        sync_persisted_plan(&mut entries, &plan);
        assert_eq!(entries.len(), 1);
        sync_persisted_plan(&mut entries, &plan);
        assert_eq!(entries.len(), 1);
        let card = decode_plan_entry(&entries[0]).unwrap();
        assert_eq!(card.plan.steps[0].status, StepStatus::InProgress);
        assert_eq!(card.plan.steps[0].text, "检查");
    }

    #[test]
    fn live_plan_updates_in_place_and_details_can_be_toggled() {
        let mut app = App::new(Vec::new(), Language::ZhCn);
        app.append_transcript(format!("WillDeep: {PLAN}"));
        app.append_transcript(format!("WillDeep: {PROGRESS}"));
        app.append_transcript(format!("WillDeep: {PROGRESS}"));
        assert_eq!(app.transcript.len(), 1);
        let card = decode_plan_entry(&app.transcript[0]).unwrap();
        assert_eq!(card.plan.steps[0].status, StepStatus::Done);
        assert_eq!(card.details.len(), 2);
        let collapsed = colored_transcript_at_width(&app.transcript, None, 24).to_string();
        assert!(collapsed.contains("2/2"));
        assert!(!collapsed.contains("没有新增状态变化"));
        assert!(app.handle_slash_command("/plan", &SkillCatalog::default()));
        let expanded = colored_transcript_at_width(&app.transcript, None, 24).to_string();
        assert!(expanded.contains("没有新增状态变化"));
        app.handle_slash_command("/plan", &SkillCatalog::default());
        assert!(!decode_plan_entry(&app.transcript[0]).unwrap().expanded);
    }

    #[test]
    fn reopened_history_matches_live_plan_state() {
        let history = [
            Message::assistant(PLAN, vec![]),
            Message::assistant(PROGRESS, vec![]),
        ];
        let mut live = Vec::new();
        append_plan_reply(&mut live, PLAN);
        append_plan_reply(&mut live, PROGRESS);
        assert_eq!(live, projected_transcript(&history, None, Language::ZhCn));
    }

    #[test]
    fn replacement_reply_keeps_card_position_and_expansion_without_old_details() {
        let replacement = "```plan\n1. 新任务\n```";
        let mut entries = vec!["You: 开始".into()];
        assert!(append_plan_reply(&mut entries, PLAN));
        assert!(append_plan_reply(&mut entries, PROGRESS));
        toggle_plan_details(&mut entries);
        entries.push("You: 继续".into());
        assert!(append_plan_reply(&mut entries, replacement));
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0], "You: 开始");
        assert_eq!(entries[2], "You: 继续");
        let card = decode_plan_entry(&entries[1]).unwrap();
        assert!(card.expanded);
        assert_eq!(card.plan.steps[0].text, "新任务");
        assert_eq!(card.details, vec![replacement]);
    }

    #[test]
    fn structured_replacement_details_match_reopened_history() {
        let call = willdeep_core::types::ToolCall {
            id: "plan-update".into(),
            name: "update_plan".into(),
            arguments: "{}".into(),
        };
        for changed_steps in [false, true] {
            for replaced in [false, true] {
                let (mut plan, _) =
                    willdeep_core::conversation::parse_plan_reply(PLAN, None).unwrap();
                plan.steps[0].status = StepStatus::Done;
                if changed_steps {
                    plan.steps[1].text = "新任务".into();
                }
                let receipt = serde_json::to_string(&willdeep_core::planning::Receipt::new(
                    plan.clone(),
                    replaced,
                ))
                .unwrap();
                let history = [
                    Message::assistant(PLAN, vec![]),
                    Message::assistant(PROGRESS, vec![]),
                    Message::assistant("", vec![call.clone()]),
                    Message::tool(&call, &receipt),
                ];
                let mut live = Vec::new();
                assert!(append_plan_reply(&mut live, PLAN));
                assert!(append_plan_reply(&mut live, PROGRESS));
                assert!(append_plan_result(&mut live, &receipt));
                let reopened = projected_transcript(&history, None, Language::ZhCn)
                    .into_iter()
                    .filter(|entry| decode_plan_entry(entry).is_some())
                    .collect::<Vec<_>>();
                assert_eq!(live, reopened);
                assert_eq!(live.len(), 1);
                let card = decode_plan_entry(&live[0]).unwrap();
                assert_eq!(card.plan, plan);
                let expected = if replaced || changed_steps {
                    Vec::<&str>::new()
                } else {
                    vec![PLAN, PROGRESS]
                };
                assert_eq!(card.details, expected);
            }
        }
    }

    #[test]
    fn persisted_replacement_matches_reopened_history_with_one_card() {
        let history = [
            Message::assistant(PLAN, vec![]),
            Message::assistant(PROGRESS, vec![]),
            Message::user("继续"),
        ];
        let (mut plan, _) = willdeep_core::conversation::parse_plan_reply(PLAN, None).unwrap();
        plan.steps[1].text = "新任务".into();
        let mut live = projected_transcript(&history, None, Language::ZhCn);
        sync_persisted_plan(&mut live, &plan);
        assert_eq!(
            live,
            projected_transcript(&history, Some(&plan), Language::ZhCn)
        );
        assert_eq!(live.len(), 2);
        assert_eq!(live[1], "You: 继续");
        let card = decode_plan_entry(&live[0]).unwrap();
        assert_eq!(card.plan, plan);
        assert!(card.details.is_empty());
    }

    #[test]
    fn narrow_terminal_renders_statuses_without_raw_fences() {
        use ratatui::backend::TestBackend;
        let entries = projected_transcript(
            &[
                Message::assistant(PLAN, vec![]),
                Message::assistant(PROGRESS, vec![]),
            ],
            None,
            Language::ZhCn,
        );
        let mut terminal = Terminal::new(TestBackend::new(24, 12)).unwrap();
        terminal
            .draw(|frame| {
                frame.render_widget(
                    Paragraph::new(colored_transcript_at_width(&entries, None, 24))
                        .wrap(Wrap { trim: false }),
                    frame.area(),
                )
            })
            .unwrap();
        let buffer = terminal.backend().buffer();
        let rendered = (0..12)
            .map(|y| (0..24).map(|x| buffer[(x, y)].symbol()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n");
        // TestBackend stores a padding cell after every wide CJK glyph.
        assert!(rendered.replace(' ', "").contains("验证接口"), "{rendered}");
        assert!(rendered.contains('✓'));
        assert!(rendered.contains('–'));
        assert!(!rendered.contains("```"));
    }

    /// 会话重开后工具行照样回放：有结果的 ✓，没结果的 …，与实时路径同一种行。
    #[test]
    fn reopened_history_replays_tool_rows_between_replies() {
        let call = |id: &str, name: &str, arguments: &str| willdeep_core::types::ToolCall {
            id: id.to_owned(),
            name: name.to_owned(),
            arguments: arguments.to_owned(),
        };
        let history = [
            Message::user("go"),
            Message::assistant(
                "Looking around first.",
                vec![call("a", "run_command", r#"{"command":"cargo test -p x"}"#)],
            ),
            Message::tool(&call("a", "run_command", "{}"), "ok"),
            Message::assistant("", vec![call("b", "read_file", r#"{"path":"x"}"#)]),
            Message::assistant("done", Vec::new()),
        ];
        assert_eq!(
            projected_transcript(&history, None, Language::En),
            vec![
                "You: go",
                "WillDeep: Looking around first.",
                "· ✓ run_command · cargo test -p x",
                "· … read_file",
                "WillDeep: done",
            ]
        );
    }

    /// 重开后附件挂在对的那句用户话下面：文本正文摊出来，图片只留可点的标签。
    #[test]
    fn reopened_history_shows_pasted_text_and_labels_attachments() {
        let mut text = Message::user("看一下这段");
        text.attachments = vec![
            MessageAttachment::Text {
                name: "paste-1.txt".to_owned(),
                content: "第一行\n第二行".to_owned(),
            },
            MessageAttachment::Image {
                name: "shot.png".to_owned(),
                media_type: "image/png".to_owned(),
                data: "AAAA".to_owned(),
                width: 16,
                height: 9,
            },
        ];
        let mut session = Session::new(std::path::PathBuf::from("/tmp"), None, "");
        session.messages = vec![text, Message::assistant("好的", Vec::new())];
        let mut app = App::new(Vec::new(), Language::ZhCn);
        app.load_transcript(&session);
        assert_eq!(app.transcript[0], "You: 看一下这段");
        assert_eq!(app.transcript_attachments.len(), app.transcript.len());
        assert_eq!(app.transcript_attachments[0].len(), 2);
        assert!(app.transcript_attachments[1].is_empty());

        app.transcript_width = 60;
        app.viewport_height = 40;
        app.prepare_transcript_frame();
        let drawn = app.transcript_rows.join("\n");
        assert!(drawn.contains("看一下这段"), "{drawn}");
        assert!(
            drawn.contains("第一行") && drawn.contains("第二行"),
            "{drawn}"
        );
        assert!(drawn.contains("附件 1 · paste-1.txt"), "{drawn}");
        assert!(drawn.contains("附件 2 · shot.png · 16×9"), "{drawn}");
        // 图片的像素数据不进聊天区。
        assert!(!drawn.contains("AAAA"), "{drawn}");

        // 标签行可点，点文本附件直接开预览。
        app.update_user_message_hits(0, 0);
        let label = app
            .attachment_hit_rows
            .iter()
            .find(|(_, _, index)| *index == 0)
            .expect("text attachment label is clickable")
            .0;
        assert!(app.open_attachment_at(label).is_some());
        assert!(app.attachment_panel.is_some());
    }

    #[test]
    fn host_instruction_never_uses_user_label() {
        let entries = projected_transcript(
            &[
                Message::host_instruction("internal prompt"),
                Message::user("internal prompt"),
            ],
            None,
            Language::ZhCn,
        );
        assert_eq!(entries[0], "System: 系统自动推进");
        assert_eq!(entries[1], "You: internal prompt");
    }
}
