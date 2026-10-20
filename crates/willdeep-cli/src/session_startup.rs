//! Project-scoped startup selection. This module only reads history; it never
//! submits a prompt, changes a checkpoint, or takes execution ownership.
use std::io;
use std::path::Path;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Result, bail};
use crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind, KeyModifiers,
    MouseButton, MouseEventKind,
};
use crossterm::{execute, terminal};
use ratatui::{
    Frame, Terminal,
    backend::CrosstermBackend,
    layout::{Constraint, Layout, Rect},
    style::{Color, Style},
    widgets::{Block, Borders, Clear, Paragraph},
};
use serde::Deserialize;
use willdeep_core::{Session, SessionDigest, SessionStore};

use crate::i18n::Language;

const COUNTDOWN: Duration = Duration::from_secs(5);
const REFRESH: Duration = Duration::from_millis(100);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "kebab-case")]
pub enum StartupMode {
    #[default]
    Ask,
    Latest,
    New,
}

impl StartupMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ask => "ask",
            Self::Latest => "latest",
            Self::New => "new",
        }
    }
}

pub fn eligible(cli: &crate::Cli, interactive: bool, run_command: bool) -> bool {
    interactive
        && !run_command
        && !cli.new_session
        && cli.resume.is_none()
        && !cli.web_input_json
        && !cli.json
        && !cli.web
}

fn latest_project(store: &SessionStore, workspace: &Path) -> Option<SessionDigest> {
    let workspace = workspace.canonicalize().ok()?;
    store
        .digests()
        .into_iter()
        .filter(|digest| !digest.bridged && digest.has_user_input)
        .filter(|digest| {
            digest
                .workspace
                .canonicalize()
                .is_ok_and(|path| path == workspace)
        })
        .max_by_key(|digest| (digest.updated_at, digest.id))
}

pub fn select(
    store: &SessionStore,
    workspace: &Path,
    mode: StartupMode,
    language: Language,
) -> Result<Option<Session>> {
    if mode == StartupMode::New {
        return Ok(None);
    }
    let Some(digest) = latest_project(store, workspace) else {
        return Ok(None);
    };
    if mode == StartupMode::Ask && !ask(&digest, language)? {
        return Ok(None);
    }
    // Reload after the chooser: a Runtime may have advanced while it was open.
    Ok(Some(store.load(digest.id)?))
}

#[derive(Default)]
struct Choice {
    new: bool,
    paused: bool,
}

impl Choice {
    fn expired(&self, elapsed: Duration) -> bool {
        !self.paused && elapsed >= COUNTDOWN
    }

    fn key(&mut self, code: KeyCode) -> Option<bool> {
        self.paused = true;
        match code {
            KeyCode::Char('n' | 'N') => Some(false),
            KeyCode::Char('y' | 'Y') => Some(true),
            KeyCode::Enter => Some(!self.new),
            KeyCode::Up
            | KeyCode::Down
            | KeyCode::Left
            | KeyCode::Right
            | KeyCode::Tab
            | KeyCode::BackTab => {
                self.new = !self.new;
                None
            }
            _ => None,
        }
    }
}

/// Always restore the terminal, including failed draws and Ctrl+C/Esc.
struct Screen;
impl Drop for Screen {
    fn drop(&mut self) {
        let _ = execute!(
            io::stdout(),
            DisableMouseCapture,
            event::DisableBracketedPaste,
            terminal::LeaveAlternateScreen,
            crossterm::cursor::Show
        );
        let _ = terminal::disable_raw_mode();
    }
}

fn ask(digest: &SessionDigest, language: Language) -> Result<bool> {
    terminal::enable_raw_mode()?;
    let _screen = Screen;
    execute!(
        io::stdout(),
        terminal::EnterAlternateScreen,
        EnableMouseCapture,
        event::EnableBracketedPaste
    )?;
    let mut term = Terminal::new(CrosstermBackend::new(io::stdout()))?;
    let started = Instant::now();
    let mut choice = Choice::default();
    loop {
        let mut hits = [Rect::default(); 2];
        term.draw(|frame| hits = render(frame, digest, language, &choice, started.elapsed()))?;
        // Input already queued at the deadline wins over the timeout.
        if event::poll(if choice.expired(started.elapsed()) {
            Duration::ZERO
        } else {
            REFRESH
        })? {
            match event::read()? {
                Event::Key(key) if key.kind == KeyEventKind::Press => {
                    if key.code == KeyCode::Esc
                        || (key.code == KeyCode::Char('c')
                            && key.modifiers.contains(KeyModifiers::CONTROL))
                    {
                        bail!(
                            "{}",
                            language.text(
                                "已取消启动",
                                "Startup cancelled",
                                "起動をキャンセルしました"
                            )
                        );
                    }
                    if let Some(resume) = choice.key(key.code) {
                        return Ok(resume);
                    }
                }
                Event::Mouse(mouse) => {
                    choice.paused = true;
                    if mouse.kind == MouseEventKind::Down(MouseButton::Left) {
                        let position = (mouse.column, mouse.row).into();
                        if hits[0].contains(position) {
                            return Ok(true);
                        }
                        if hits[1].contains(position) {
                            return Ok(false);
                        }
                    }
                }
                Event::Paste(_) | Event::FocusLost => choice.paused = true,
                _ => {}
            }
        } else if choice.expired(started.elapsed()) {
            return Ok(true);
        }
    }
}

fn render(
    frame: &mut Frame,
    digest: &SessionDigest,
    language: Language,
    choice: &Choice,
    elapsed: Duration,
) -> [Rect; 2] {
    let screen = frame.area();
    let width = screen.width.min(90);
    let height = screen.height.min(14);
    let area = Rect::new(
        screen.x + (screen.width - width) / 2,
        screen.y + (screen.height - height) / 2,
        width,
        height,
    );
    let block = Block::default().borders(Borders::ALL).title(language.text(
        " 继续本项目的会话？ ",
        " Continue this project's session? ",
        " このプロジェクトの会話を続けますか？ ",
    ));
    let inner = block.inner(area);
    frame.render_widget(Clear, area);
    frame.render_widget(block, area);
    // Reserve the actions first, so very short terminals still have usable rows.
    let rows = Layout::vertical([
        Constraint::Min(0),
        Constraint::Length(1),
        Constraint::Length(u16::from(inner.height >= 7)),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .split(inner);
    let age = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        .saturating_sub(digest.updated_at)
        / 60;
    let metadata = format!(
        "{}\n{}\n{}",
        digest.title,
        digest.workspace.display(),
        language.pick(
            format!("最后使用：{age} 分钟前"),
            format!("Last used: {age} minutes ago"),
            format!("最終使用：{age} 分前")
        )
    );
    frame.render_widget(Paragraph::new(metadata), rows[0]);
    for (index, text) in [
        language.text(
            "Y  继续上次会话",
            "Y  Continue previous session",
            "Y  前の会話を続ける",
        ),
        language.text("N  新建会话", "N  New session", "N  新しい会話"),
    ]
    .into_iter()
    .enumerate()
    {
        let selected = choice.new == (index == 1);
        let style = if selected {
            Style::default().bg(Color::Cyan).fg(Color::Black)
        } else {
            Style::default()
        };
        frame.render_widget(
            Paragraph::new(format!("{} {text}", if selected { "▶" } else { " " })).style(style),
            rows[if index == 0 { 1 } else { 3 }],
        );
    }
    let status = if choice.paused {
        language
            .text(
                "倒计时已暂停 · 方向键选择，Enter 确认 · Esc 退出",
                "Countdown paused · Arrows select, Enter confirms · Esc exits",
                "カウント停止 · 矢印で選択、Enterで確定 · Escで終了",
            )
            .to_owned()
    } else {
        let seconds = COUNTDOWN.as_secs().saturating_sub(elapsed.as_secs());
        language.pick(
            format!("{seconds} 秒后继续上次会话 · 操作后停止倒计时"),
            format!("Continue in {seconds}s · Interacting stops the countdown"),
            format!("{seconds} 秒後に続行 · 操作でカウント停止"),
        )
    };
    frame.render_widget(Paragraph::new(status), rows[4]);
    frame.render_widget(
        Paragraph::new("willdeep config startup ask | latest | new"),
        rows[5],
    );
    [rows[1], rows[3]]
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn explicit_intent_bypasses_chooser() {
        for args in [
            vec!["willdeep", "--new-session"],
            vec!["willdeep", "--resume"],
            vec!["willdeep", "--json"],
            vec!["willdeep", "--web-input-json"],
        ] {
            let cli = crate::Cli::try_parse_from(args).unwrap();
            assert!(!eligible(&cli, true, false));
        }
        let cli = crate::Cli::try_parse_from(["willdeep"]).unwrap();
        assert!(eligible(&cli, true, false));
        assert!(!eligible(&cli, false, false));
        assert!(!eligible(&cli, true, true));
        assert!(crate::Cli::try_parse_from(["willdeep", "--new-session", "--resume"]).is_err());
    }

    #[test]
    fn interaction_stops_timeout_and_new_selection_controls_enter() {
        let mut choice = Choice::default();
        assert!(choice.expired(COUNTDOWN));
        assert_eq!(choice.key(KeyCode::Down), None);
        assert!(!choice.expired(COUNTDOWN * 10));
        assert_eq!(choice.key(KeyCode::Enter), Some(false));
        assert_eq!(choice.key(KeyCode::Char('Y')), Some(true));
    }

    #[test]
    fn configuration_save_preserves_comments_profiles_and_validates_mode() {
        let root =
            std::env::temp_dir().join(format!("willdeep-startup-config-{}", uuid::Uuid::new_v4()));
        let path = root.join("config.toml");
        crate::model_routing::save_session_startup(&path, StartupMode::Latest).unwrap();
        assert_eq!(
            crate::config::LoadedConfig::load(Some(&path))
                .unwrap()
                .file
                .agent
                .session_startup,
            StartupMode::Latest
        );
        let text = "# preserve\n[agent]\nsession_startup = 'latest' # preference\n[providers.test]\napi_key_env = 'FIXTURE_ONLY'\nmodel = 'test-model'\n";
        std::fs::write(&path, text).unwrap();
        crate::model_routing::save_session_startup(&path, StartupMode::New).unwrap();
        let updated = std::fs::read_to_string(&path).unwrap();
        assert!(updated.contains("# preserve"));
        assert!(updated.contains("# preference"));
        assert!(updated.contains("api_key_env = 'FIXTURE_ONLY'"));
        assert_eq!(
            crate::config::LoadedConfig::load(Some(&path))
                .unwrap()
                .file
                .agent
                .session_startup,
            StartupMode::New
        );
        assert!(
            toml::from_str::<crate::config::ConfigFile>("[agent]\nsession_startup = 'invalid'")
                .is_err()
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn layout_has_single_line_actions_and_inert_gap_in_all_languages() {
        let digest = SessionDigest {
            id: uuid::Uuid::new_v4(),
            title: "previous task".into(),
            workspace: "/project".into(),
            created_at: 1,
            updated_at: 1,
            pinned_at: None,
            has_user_input: true,
            message_count: 2,
            preview: None,
            bridged: false,
        };
        for language in [Language::ZhCn, Language::En, Language::Ja] {
            for (width, height) in [(100, 30), (32, 10), (20, 5)] {
                let mut term =
                    Terminal::new(ratatui::backend::TestBackend::new(width, height)).unwrap();
                let mut hits = [Rect::default(); 2];
                term.draw(|frame| {
                    hits = render(frame, &digest, language, &Choice::default(), Duration::ZERO)
                })
                .unwrap();
                assert!(hits.iter().all(|hit| hit.height <= 1));
                assert!(hits[0].bottom() <= hits[1].y);
                if height >= 10 {
                    assert_eq!(hits[1].y - hits[0].bottom(), 1);
                }
            }
        }
    }

    #[test]
    fn workspace_filter_ignores_newer_other_project_and_empty_sessions() {
        let root = std::env::temp_dir().join(format!("willdeep-startup-{}", uuid::Uuid::new_v4()));
        let project = root.join("project");
        let other = root.join("other");
        std::fs::create_dir_all(&project).unwrap();
        std::fs::create_dir_all(&other).unwrap();
        let store = SessionStore::new(&root);
        assert!(
            select(&store, &project, StartupMode::Ask, Language::En)
                .unwrap()
                .is_none()
        );
        let mut wanted = Session::new(project.clone(), None, "project history");
        wanted
            .messages
            .push(willdeep_core::Message::user("previous question"));
        store.save(&mut wanted).unwrap();
        let wanted_path = root.join("sessions").join(format!("{}.json", wanted.id));
        let mut value = serde_json::to_value(&wanted).unwrap();
        value["updated_at"] = 1.into();
        std::fs::write(&wanted_path, serde_json::to_vec(&value).unwrap()).unwrap();
        let mut unrelated = Session::new(other, None, "different project");
        unrelated
            .messages
            .push(willdeep_core::Message::user("other question"));
        store.save(&mut unrelated).unwrap();
        let mut blank = Session::new(project.clone(), None, "");
        store.save(&mut blank).unwrap();
        let before =
            std::fs::read(root.join("sessions").join(format!("{}.json", wanted.id))).unwrap();
        assert_eq!(latest_project(&store, &project).unwrap().id, wanted.id);
        assert!(
            select(&store, &project, StartupMode::New, Language::En)
                .unwrap()
                .is_none()
        );
        assert_eq!(
            select(&store, &project, StartupMode::Latest, Language::En)
                .unwrap()
                .unwrap()
                .id,
            wanted.id
        );
        assert_eq!(
            std::fs::read(root.join("sessions").join(format!("{}.json", wanted.id))).unwrap(),
            before
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}
