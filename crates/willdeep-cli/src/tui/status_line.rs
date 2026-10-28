use super::*;

impl App {
    fn take_status_notice(&mut self) -> Option<String> {
        if let Some((held_notice, deadline)) = &self.notice_hold
            && self.notice.as_deref() == Some(held_notice.as_str())
            && Instant::now() < *deadline
        {
            return self.notice.clone();
        }
        self.notice_hold = None;
        self.notice.take()
    }

    pub(super) fn status_line(&mut self) -> String {
        let status = if self.native_selection_mode {
            self.take_status_notice().unwrap_or_else(|| {
                self.language
                    .text(
                        "终端原生选择 · 拖选后可用右键复制 · Esc 恢复 WillDeep 鼠标操作",
                        "Native terminal selection · drag, then right-click to copy · Esc restores WillDeep mouse controls",
                        "端末の標準選択 · ドラッグ後に右クリックでコピー · Esc でマウス操作を復元",
                    )
                    .to_owned()
            })
        } else if self.selection_mode {
            self.take_status_notice().unwrap_or_else(|| {
                self.language
                    .text(
                        "文本选择模式 · 鼠标拖选 · Ctrl/Cmd+C 或 Y 复制 · Q 引用 · Esc 退出",
                        "Text selection · drag · Ctrl/Cmd+C or Y copy · Q quote · Esc exits",
                        "テキスト選択 · ドラッグ · Ctrl/Cmd+C / Y コピー · Q 引用 · Esc 終了",
                    )
                    .to_owned()
            })
        } else {
            self.take_status_notice().unwrap_or_else(|| {
            let input_tokens = self.latest_usage.input_tokens.unwrap_or(0);
            let output_tokens = self.latest_usage.output_tokens.unwrap_or(0);
            let input = format_token_count(input_tokens);
            let output = format_token_count(output_tokens);
            let cache = cache_hit_rate(&self.latest_usage)
                .map(|rate| {
                    format!(
                        " · {} {rate:.2}%",
                        self.language.text("缓存", "cache", "キャッシュ")
                    )
                })
                .unwrap_or_default();
            let context_tokens = self.context_tokens.max(input_tokens);
            let context_pct = context_tokens.saturating_mul(100) / self.context_window.max(1);
            let elapsed = format_elapsed_span(
                self.turn_started
                    .map(|value| value.elapsed())
                    .or(self.last_elapsed)
                    .unwrap_or_default()
                    .as_secs_f32(),
                1,
            );
            if self.running {
                format!(
                    "{} · {}: {} · {} {context_pct}% · {} ↑{input} ↓{output}{cache}{queued} · Esc {} · F1",
                    self.working_summary().unwrap_or_else(|| self.language.text("运行中", "Running", "実行中").to_owned()),
                    self.language.text("焦点", "Focus", "フォーカス"),
                    focus_label(self.focus, self.language),
                    self.language.text("上下文", "context", "コンテキスト"),
                    self.language.text("最近", "latest", "直近"),
                    // 运行中最该被看见的是「怎么停下来」，不是文本选择的快捷键。
                    self.language.text("Esc×2 中断", "Esc×2 interrupt", "Esc×2 中断"),
                    queued = if self.queued_prompts.is_empty() {
                        String::new()
                    } else {
                        format!(" · {} {}", self.language.text("待发", "queued", "送信待ち"), self.queued_prompts.len())
                    }
                )
            } else {
                format!(
                    "{} · {}: {} · {} {context_pct}% · {} ↑{input} ↓{output}{cache} · {elapsed} · {} · Ctrl+S {} · F1",
                    if self.queued_prompts.is_empty() {
                        self.language.text("就绪", "Ready", "準備完了")
                    } else {
                        self.language.text("待发消息等待发送", "Queued messages waiting to send", "送信待ちがあります")
                    },
                    self.language.text("焦点", "Focus", "フォーカス"),
                    focus_label(self.focus, self.language),
                    self.language.text("上下文", "context", "コンテキスト"),
                    self.language.text("最近", "latest", "直近"),
                    self.language.text(
                        "Enter 发送 · Cmd+Enter 入队",
                        "Enter sends · Cmd+Enter queues",
                        "Enter で送信 · Cmd+Enter で追加",
                    ),
                    self.language.text("选择", "select", "選択")
                )
            }
        })
        };
        if self.peer_unread > 0 {
            format!(
                "{} {} · /peers | {status}",
                self.language.text("协作未读", "Peer unread", "連携未読"),
                self.peer_unread
            )
        } else {
            status
        }
    }
}
