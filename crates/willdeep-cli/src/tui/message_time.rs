//! Display-only message times. Old Runtime processes need no schema changes.
use super::narration::FoldedTranscript;
use super::*;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

const MINUTE: u64 = 60;
const HOUR: u64 = 60 * MINUTE;
const DAY: u64 = 24 * HOUR;

pub(super) fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

pub(super) fn relative_time(sent: u64, current: u64, language: Language) -> String {
    let age = current.saturating_sub(sent);
    if age < MINUTE {
        return language.text("刚刚", "now", "たった今").to_owned();
    }
    let (count, unit) = if age < HOUR {
        (age / MINUTE, language.text("分前", "m ago", "分前"))
    } else if age < DAY {
        (age / HOUR, language.text("小时前", "h ago", "時間前"))
    } else {
        (age / DAY, language.text("天前", "d ago", "日前"))
    };
    format!("{count}{unit}")
}

fn is_message(row: &str) -> bool {
    row.starts_with("You: ") || row.starts_with("WillDeep: ")
}

fn key(row: &str) -> String {
    format!("{:x}", Sha256::digest(row.as_bytes()))
}

#[derive(Default)]
pub(super) struct MessageTimes {
    rows: Vec<Option<u64>>,
    // Hashes and duplicate ordinals, never message bodies or model context.
    known: HashMap<String, Vec<Option<u64>>>,
    path: Option<PathBuf>,
    dirty: bool,
    offsets: HashMap<String, usize>,
    pub(super) event_time: Option<u64>,
}

impl MessageTimes {
    pub(super) fn reserve_delivery(
        &mut self,
        transcript: &[String],
        text: &str,
        pending_copies: usize,
        sent_at: u64,
    ) {
        let row = terminal_safe_text(&format!("You: {text}"));
        let digest = key(&row);
        let ordinal = transcript
            .iter()
            .filter(|previous| **previous == row)
            .count()
            + pending_copies
            + self.offsets.get(&digest).copied().unwrap_or_default();
        let times = self.known.entry(digest).or_default();
        times.resize(times.len().max(ordinal + 1), None);
        times[ordinal] = Some(sent_at);
        self.dirty = true;
    }
    pub(super) fn recording_time(&self) -> u64 {
        self.event_time
            .filter(|value| *value > 0)
            .unwrap_or_else(now)
    }
    fn load(&mut self, path: PathBuf, transcript: &[String]) -> Result<()> {
        let previous_error = self.flush().err();
        self.rows.clear();
        self.known.clear();
        self.offsets.clear();
        self.dirty = false;
        self.path = Some(path.clone());
        self.known = match std::fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes).context("read TUI message times")?,
            Err(error) if error.kind() == io::ErrorKind::NotFound => HashMap::new(),
            Err(error) => return Err(error).context("read TUI message times"),
        };
        let mut ordinals = HashMap::<String, usize>::new();
        self.rows = transcript
            .iter()
            .map(|row| {
                if !is_message(row) {
                    return None;
                }
                let digest = key(row);
                let ordinal = ordinals.entry(digest.clone()).or_default();
                let time = self
                    .known
                    .get(&digest)
                    .and_then(|times| times.get(*ordinal))
                    .copied()
                    .flatten();
                *ordinal += 1;
                time
            })
            .collect();
        if let Some(error) = previous_error {
            return Err(error);
        }
        Ok(())
    }

    pub(super) fn observe(&mut self, transcript: &[String], current: u64) {
        self.rows.resize(transcript.len(), None);
        let Some((last, previous)) = transcript.split_last() else {
            return;
        };
        if !is_message(last) {
            return;
        }
        let digest = key(last);
        let ordinal = previous.iter().filter(|row| *row == last).count()
            + self.offsets.get(&digest).copied().unwrap_or_default();
        let times = self.known.entry(digest).or_default();
        times.resize(times.len().max(ordinal + 1), None);
        times[ordinal] = Some(current);
        self.rows[previous.len()] = Some(current);
        self.dirty = true;
    }

    pub(super) fn remove(&mut self, index: usize) {
        if index < self.rows.len() {
            self.rows.remove(index);
        }
    }

    pub(super) fn clear_rows(&mut self, transcript: &[String]) {
        for row in transcript.iter().filter(|row| is_message(row)) {
            *self.offsets.entry(key(row)).or_default() += 1;
        }
        self.rows.clear();
    }

    pub(super) fn time_at(&self, index: usize) -> Option<u64> {
        self.rows.get(index).copied().flatten()
    }

    pub(super) fn labels(
        &self,
        count: usize,
        language: Language,
        current: u64,
    ) -> Vec<Option<String>> {
        (0..count)
            .map(|index| {
                self.rows
                    .get(index)
                    .copied()
                    .flatten()
                    .map(|sent| relative_time(sent, current, language))
            })
            .collect()
    }

    fn flush(&mut self) -> Result<()> {
        if !self.dirty {
            return Ok(());
        }
        let Some(path) = &self.path else {
            return Ok(());
        };
        std::fs::create_dir_all(path.parent().expect("message time directory"))?;
        let temporary = path.with_extension(format!("{}.tmp", uuid::Uuid::new_v4()));
        let bytes = serde_json::to_vec(&self.known)?;
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temporary)?;
        std::io::Write::write_all(&mut file, &bytes)?;
        drop(file);
        if let Err(error) = std::fs::rename(&temporary, path) {
            let _ = std::fs::remove_file(&temporary);
            return Err(error).context("save TUI message times");
        }
        self.dirty = false;
        Ok(())
    }
}

impl App {
    pub(super) fn reload_message_times(&mut self, id: uuid::Uuid) {
        let Some(path) = self
            .message_times
            .path
            .as_ref()
            .map(|path| path.with_file_name(format!("{id}.json")))
        else {
            return;
        };
        if let Err(error) = self.message_times.load(path, &self.transcript) {
            self.message_time_error(error);
        }
    }
    pub(super) fn load_message_times(&mut self, home: &Path, id: uuid::Uuid) {
        let path = home.join("tui-message-times").join(format!("{id}.json"));
        if let Err(error) = self.message_times.load(path, &self.transcript) {
            self.message_time_error(error);
        }
    }

    pub(super) fn save_message_times(&mut self) {
        if let Err(error) = self.message_times.flush() {
            self.message_time_error(error);
        }
    }

    pub(super) fn append_transcript_at(&mut self, row: String, sent_at: u64) {
        self.append_transcript(row);
        self.message_times.observe(&self.transcript, sent_at);
        self.refresh_transcript_height();
    }

    pub(super) fn timed_transcript_height(&self) -> usize {
        let folded = self.display_transcript();
        let labels = self.message_time_labels(&folded, now());
        render_with_times(
            &folded.rows,
            &labels,
            None,
            self.transcript_width,
            self.language,
        )
        .lines
        .len()
    }

    fn message_time_error(&mut self, error: anyhow::Error) {
        self.notice = Some(format!(
            "{}: {error:#}",
            self.language.text(
                "消息时间保存或读取失败",
                "Could not save or read message times",
                "メッセージ時刻の保存・読込に失敗しました"
            )
        ));
    }

    pub(super) fn message_time_labels(
        &self,
        folded: &FoldedTranscript,
        current: u64,
    ) -> Vec<Option<String>> {
        let mut labels = vec![None; folded.rows.len()];
        for (index, row) in self.transcript.iter().enumerate() {
            if is_message(row)
                && let Some(Some(sent)) = self.message_times.rows.get(index)
                && let Some(target) = folded
                    .index_of
                    .get(index)
                    .and_then(|index| labels.get_mut(*index))
            {
                *target = Some(relative_time(*sent, current, self.language));
            }
        }
        labels
    }
}

pub(super) fn render_with_times(
    rows: &[String],
    labels: &[Option<String>],
    search: Option<&str>,
    width: usize,
    language: Language,
) -> Text<'static> {
    let last = rows.len().saturating_sub(1);
    let mut text = Text::default();
    for (index, row) in rows.iter().enumerate() {
        // 这里按行画，调用方可能只传入到某个命中为止的切片，所以「轮到你」只在
        // 真正画整段聊天区的最后一行时才补上。
        // 单行渲染自己不补「轮到你」：那是整段聊天区最后一行的事，这里补了一次，
        // 里面再补一次就会叠成两句。
        let displayed = divider_without_your_turn(row, language);
        let mut entry = wrap_styled_text(
            colored_transcript_at_width(std::slice::from_ref(&displayed), search, width),
            width,
        );
        if let Some(Some(label)) = labels.get(index) {
            let suffix = format!(" · {label}");
            let suffix_width = UnicodeWidthStr::width(suffix.as_str());
            if suffix_width <= width
                && let Some(last) = entry.lines.iter().rposition(|line| line.width() > 0)
            {
                let span = Span::styled(
                    suffix,
                    Style::default().fg(Color::Gray).add_modifier(Modifier::DIM),
                );
                if entry.lines[last].width() + suffix_width <= width {
                    entry.lines[last].spans.push(span);
                } else {
                    entry.lines.insert(last + 1, Line::from(span));
                }
            }
        }
        // 放在时间标签之后：时间标签按原行宽决定贴在行尾还是另起一行，
        // 「轮到你」不能先把行撑宽，否则窄屏上时间会被挤到下一行。
        if index == last && displayed.starts_with(TURN_DIVIDER_PREFIX) {
            let label = format!(" · {}", your_turn_label(language));
            if let Some(line) = entry.lines.iter_mut().rev().find(|line| line.width() > 0) {
                line.push_span(Span::raw(label));
            }
        }
        text.lines.extend(entry.lines);
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_times_cover_boundaries_languages_and_clock_skew() {
        for (age, zh, en, ja) in [
            (0, "刚刚", "now", "たった今"),
            (59, "刚刚", "now", "たった今"),
            (60, "1分前", "1m ago", "1分前"),
            (3600, "1小时前", "1h ago", "1時間前"),
            (3 * DAY, "3天前", "3d ago", "3日前"),
        ] {
            for (language, expected) in
                [(Language::ZhCn, zh), (Language::En, en), (Language::Ja, ja)]
            {
                assert_eq!(relative_time(100, 100 + age, language), expected);
            }
        }
        assert_eq!(relative_time(200, 100, Language::En), "now");
    }

    #[test]
    fn times_survive_reopening_without_inventing_history_or_losing_duplicates() {
        let root = std::env::temp_dir().join(format!("tui-times-{}", uuid::Uuid::new_v4()));
        let path = root.join("times.json");
        let mut times = MessageTimes::default();
        let mut rows = vec!["You: legacy".to_owned()];
        times.load(path.clone(), &rows).unwrap();
        assert_eq!(times.rows, [None]);
        rows.push("You: again".into());
        times.observe(&rows, 100);
        rows.push("You: again".into());
        times.observe(&rows, 200);
        times.flush().unwrap();
        let saved = std::fs::read_to_string(&path).unwrap();
        assert!(!saved.contains("again"));
        times.load(path, &rows).unwrap();
        assert_eq!(times.rows, [None, Some(100), Some(200)]);
        times.remove(1);
        assert_eq!(times.rows, [None, Some(200)]);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn timestamps_do_not_change_markdown_tables_code_or_overflow_narrow_windows() {
        for body in [
            "You: 中文",
            "WillDeep: ```rust\nlet x = 1;\n```",
            "WillDeep: | A | B |\n|---|---|\n| 1 | 2 |",
        ] {
            for width in [8, 30, 80] {
                let rendered = render_with_times(
                    &[body.into()],
                    &[Some("1分前".into())],
                    None,
                    width,
                    Language::ZhCn,
                );
                assert!(rendered.lines.iter().all(|line| line.width() <= width));
                assert!(
                    text_rows(&rendered)
                        .iter()
                        .any(|line| line.contains("1分前"))
                );
            }
        }
    }

    #[test]
    fn clearing_the_view_does_not_overwrite_an_older_identical_message_time() {
        let mut times = MessageTimes::default();
        let mut rows = vec!["You: again".to_owned()];
        times.observe(&rows, 100);
        times.clear_rows(&rows);
        rows.clear();
        rows.push("You: again".into());
        times.observe(&rows, 200);
        assert_eq!(times.known[&key("You: again")], [Some(100), Some(200)]);
    }

    #[test]
    fn queued_delivery_and_model_read_preserve_the_original_message_time() {
        let mut app = App::new(Vec::new(), Language::ZhCn);
        app.input.insert("status?");
        app.queue_submitted_prompt();
        app.message_times.observe(&app.transcript, 100);
        app.note_steering_delivered("status?", app.queued_submission_time(0));
        app.remove_queued_prompt_row(0);
        app.queued_prompts.pop_front();
        app.note_model_step_started();
        assert_eq!(app.transcript, ["You: status?"]);
        assert_eq!(app.message_times.time_at(0), Some(100));
        let folded = app.display_transcript();
        assert_eq!(
            app.message_time_labels(&folded, 160),
            [Some("1分前".into())]
        );
    }

    #[test]
    fn age_labels_refresh_while_the_cached_message_source_stays_unchanged() {
        let mut app = App::new(Vec::new(), Language::En);
        app.append_transcript_at("You: still here".into(), now());
        app.prepare_transcript_frame();
        assert!(app.transcript_rows.iter().any(|row| row.contains("now")));
        app.message_times
            .observe(&app.transcript, now().saturating_sub(3 * DAY));
        app.prepare_transcript_frame();
        assert!(app.transcript_rows.iter().any(|row| row.contains("3d ago")));
        assert_eq!(app.transcript, ["You: still here"]);
    }

    #[test]
    fn replayed_runtime_message_uses_event_time_and_does_not_backdate_new_input() {
        let root = std::env::temp_dir().join(format!("tui-event-times-{}", uuid::Uuid::new_v4()));
        let store = SessionStore::new(&root);
        let mut session = Session::new(root.clone(), None, "message times");
        session.runtime_managed = true;
        let mut app = App::new(Vec::new(), Language::En);
        app.begin_turn(true, "replaying".into());
        runtime_ui::apply_runtime_events(
            &mut app,
            vec![crate::daemon::RemoteRuntimeEvent {
                sequence: 1,
                timestamp: 100,
                kind: "task.output".into(),
                message: "task_id=test {\"type\":\"assistant_text\",\"text\":\"earlier reply\"}"
                    .into(),
                visible: true,
                session_id: Some(session.id),
            }],
            &mut session,
            &store,
        )
        .unwrap();
        assert_eq!(app.message_times.time_at(0), Some(100));
        assert_eq!(app.message_times.event_time, None);
        app.append_transcript("You: new input".into());
        assert!(app.message_times.time_at(1).unwrap() > 100);
    }

    #[test]
    fn selected_duplicate_keeps_its_time_even_if_tui_reopens_before_model_read() {
        let root =
            std::env::temp_dir().join(format!("tui-delivery-times-{}", uuid::Uuid::new_v4()));
        let id = uuid::Uuid::new_v4();
        let mut app = App::new(Vec::new(), Language::En);
        app.load_message_times(&root, id);
        for sent_at in [100, 200] {
            app.input.insert("again");
            app.queue_submitted_prompt();
            app.message_times.observe(&app.transcript, sent_at);
        }
        assert_eq!(app.queued_submission_time(1), 200);
        app.note_steering_delivered("again", app.queued_submission_time(1));
        app.remove_queued_prompt_row(1);
        app.queued_prompts.remove(1);
        app.message_times.flush().unwrap();
        let mut reopened = App::new(vec!["You: again".into()], Language::En);
        reopened.load_message_times(&root, id);
        assert_eq!(reopened.message_times.time_at(0), Some(200));
        app.note_model_step_started();
        assert_eq!(app.message_times.time_at(0), Some(100));
        assert_eq!(app.message_times.time_at(1), Some(200));
        std::fs::remove_dir_all(root).unwrap();
    }
}
