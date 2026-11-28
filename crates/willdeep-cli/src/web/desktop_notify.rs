//! System notifications for the desktop shell.
//!
//! The shell is a Tauri process that only knows this adapter by the JSON line
//! it prints on startup. There is no channel back, so a notification rides the
//! same private stdout pipe as one more JSON line. The shell turns it into a
//! system notification; this process never talks to the windowing system.
//!
//! What gets announced is the same two moments the TUI rings its bell for: a
//! task that reached a result (`task.completed`, `task.partial`) and a task
//! that parked on a person (`task.waiting_approval`, `task.waiting_answer`).
//! The matching `turn.*` events describe the same moment and are ignored, as
//! are failures, which stay in the transcript where the reason can be read.
//!
//! On macOS and Linux a TUI that is open on the same session has already rung,
//! so the line is not printed. Windows terminals do not surface that bell, so
//! the notification goes out regardless.

use std::collections::{HashMap, VecDeque};
use std::io::Write;
use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result};
use serde::Serialize;
use uuid::Uuid;
use willdeep_runtime_protocol::{ApiResponse, RuntimeEvent};

use crate::daemon::tui_presence;
use crate::i18n::Language;

const TITLE_LIMIT: usize = 80;
const RETRY_DELAY: Duration = Duration::from_secs(2);
const LOOKUP_TIMEOUT: Duration = Duration::from_secs(5);
const LOOKUP_RETRIES: usize = 4;
const LOOKUP_RETRY_DELAY: Duration = Duration::from_millis(200);

/// The four moments worth pulling someone back to the window for, and the word
/// the desktop shell sees for each. Everything else, including the `turn.*`
/// twin of a `task.*` event, is not a notification.
fn kind_of(event: &str) -> Option<&'static str> {
    Some(match event {
        "task.completed" => "completed",
        "task.partial" => "partial",
        "task.waiting_approval" => "approval",
        "task.waiting_answer" => "question",
        _ => return None,
    })
}

/// `task_id=<uuid>` is the one field every one of these events carries. The
/// approval and question events have nothing else.
fn task_id(message: &str) -> Option<Uuid> {
    message
        .split_whitespace()
        .find_map(|part| part.strip_prefix("task_id="))
        .and_then(|value| value.parse().ok())
}

#[derive(Serialize, PartialEq, Eq, Debug)]
struct Notice {
    willdeep_desktop_notice: NoticeBody,
}

#[derive(Serialize, PartialEq, Eq, Debug)]
struct NoticeBody {
    kind: &'static str,
    title: String,
    body: String,
}

/// What one event is worth saying. `Quiet` is a real decision, not a failure:
/// a TUI already rang for it, or it was announced before.
#[derive(PartialEq, Eq, Debug)]
enum Decision {
    Announce(Notice),
    Quiet,
}

/// Remembers which task it already announced, so a daemon that replays its
/// backlog on every reconnect does not notify twice for the same moment.
struct Seen {
    tasks: VecDeque<Uuid>,
}

impl Seen {
    fn fresh(&mut self, task: Uuid) -> bool {
        if self.tasks.contains(&task) {
            return false;
        }
        self.tasks.push_back(task);
        if self.tasks.len() > 256 {
            self.tasks.pop_front();
        }
        true
    }
}

/// Follows the runtime event stream for as long as the desktop adapter runs.
/// Every failure just waits and tries again: a missed notification is a missed
/// ping, never a reason to take the window down.
pub(super) fn spawn(home: std::path::PathBuf, language: Language) {
    tokio::spawn(async move {
        let mut seen = Seen {
            tasks: VecDeque::new(),
        };
        let mut sessions = HashMap::new();
        let mut after = 0_u64;
        loop {
            if follow_once(&home, language, &mut seen, &mut sessions, &mut after)
                .await
                .is_err()
            {
                tokio::time::sleep(RETRY_DELAY).await;
            }
        }
    });
}

async fn follow_once(
    home: &Path,
    language: Language,
    seen: &mut Seen,
    sessions: &mut HashMap<Uuid, Option<Uuid>>,
    after: &mut u64,
) -> Result<()> {
    let mut stream = crate::daemon::desktop_event_client(home)?
        .stream_local_events(*after, 1_000)
        .await?;
    while let Some(event) = stream.next_event().await? {
        if kind_of(&event.kind).is_none() {
            *after = (*after).max(event.sequence);
            continue;
        }
        let Some(task) = task_id(&event.message) else {
            *after = (*after).max(event.sequence);
            continue;
        };
        let session = match sessions.get(&task) {
            Some(session) => *session,
            None => {
                // A task can be announced before its record is readable. Do not
                // remember a miss, and do not move the cursor past it.
                let Some(session) = lookup_session(home, task).await? else {
                    return Ok(());
                };
                sessions.insert(task, session);
                session
            }
        };
        if let Decision::Announce(notice) = decide(home, language, seen, &event, session) {
            emit(&notice)?;
        }
        *after = (*after).max(event.sequence);
    }
    Ok(())
}

/// `None` means the task was not there yet. `Some(None)` means it is there and
/// belongs to no session.
async fn lookup_session(home: &Path, task: Uuid) -> Result<Option<Option<Uuid>>> {
    for attempt in 0..LOOKUP_RETRIES {
        let client = crate::daemon::desktop_event_client(home)?;
        let response = tokio::time::timeout(LOOKUP_TIMEOUT, client.task(task))
            .await
            .context("look up the task behind a desktop notification")??;
        match response {
            ApiResponse::Ok { data, .. } => return Ok(Some(data.session_id)),
            ApiResponse::Error { error, .. }
                if error.code == willdeep_runtime_protocol::ErrorCode::NotFound => {}
            ApiResponse::Error { error, .. } => {
                return Err(anyhow::Error::from(error)
                    .context("look up the task behind a desktop notification"));
            }
        }
        if attempt + 1 < LOOKUP_RETRIES {
            tokio::time::sleep(LOOKUP_RETRY_DELAY).await;
        }
    }
    Ok(None)
}

fn decide(
    home: &Path,
    language: Language,
    seen: &mut Seen,
    event: &RuntimeEvent,
    session: Option<Uuid>,
) -> Decision {
    let Some(kind) = kind_of(&event.kind) else {
        return Decision::Quiet;
    };
    let Some(task) = task_id(&event.message) else {
        return Decision::Quiet;
    };
    if !seen.fresh(task) {
        return Decision::Quiet;
    }
    // A TUI open on this session already rang. Windows never counts as covered:
    // its terminals do not deliver the bell.
    if !cfg!(windows) && session.is_some_and(|session| tui_presence::watching(home, session)) {
        return Decision::Quiet;
    }
    Decision::Announce(Notice {
        willdeep_desktop_notice: NoticeBody {
            kind,
            title: session_title(home, session),
            body: body_text(language, kind).to_owned(),
        },
    })
}

fn session_title(home: &Path, session: Option<Uuid>) -> String {
    let Some(session) = session else {
        return String::new();
    };
    willdeep_core::SessionStore::new(home)
        .load(session)
        .map(|session| clamp(&session.title))
        .unwrap_or_default()
}

fn clamp(title: &str) -> String {
    let title = title.split_whitespace().collect::<Vec<_>>().join(" ");
    let count = title.chars().count();
    if count <= TITLE_LIMIT {
        return title;
    }
    let mut end = title.len();
    for (index, (byte, _)) in title.char_indices().rev().enumerate() {
        if count - index - 1 <= TITLE_LIMIT {
            end = byte;
            break;
        }
    }
    format!("{}…", &title[..end])
}

fn body_text(language: Language, kind: &str) -> &'static str {
    match kind {
        "completed" => language.text(
            "这一轮做完了",
            "The turn finished",
            "このターンが完了しました",
        ),
        "partial" => language.text(
            "这一轮先做到这里",
            "The turn stopped partway",
            "このターンは途中で止まりました",
        ),
        "approval" => language.text(
            "有一项操作等你确认",
            "Something is waiting for your approval",
            "確認待ちの操作があります",
        ),
        "question" => language.text(
            "有一个问题等你回答",
            "A question is waiting for you",
            "回答待ちの質問があります",
        ),
        _ => "",
    }
}

fn emit(notice: &Notice) -> Result<()> {
    let line = serde_json::to_string(notice)?;
    let mut output = std::io::stdout().lock();
    writeln!(output, "{line}")?;
    output.flush()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(kind: &str, task: Uuid) -> RuntimeEvent {
        RuntimeEvent {
            sequence: 1,
            timestamp: 0,
            kind: kind.to_owned(),
            message: format!("task_id={task} error=secret token"),
        }
    }

    fn home() -> std::path::PathBuf {
        let home = std::env::temp_dir().join(format!("desktop-notify-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&home).unwrap();
        home
    }

    #[test]
    fn only_the_bell_moments_notify() {
        assert_eq!(kind_of("task.completed"), Some("completed"));
        assert_eq!(kind_of("task.partial"), Some("partial"));
        assert_eq!(kind_of("task.waiting_approval"), Some("approval"));
        assert_eq!(kind_of("task.waiting_answer"), Some("question"));
        for ignored in [
            "task.failed",
            "task.cancelled",
            "task.interrupted",
            "task.output",
            "turn.completed",
            "turn.partial",
            "turn.failed",
            "tool.interrupted",
        ] {
            assert_eq!(kind_of(ignored), None, "{ignored}");
        }
    }

    #[test]
    fn the_task_id_comes_out_of_the_message_and_nothing_else_does() {
        let id = Uuid::new_v4();
        assert_eq!(
            task_id(&format!("task_id={id} session_id=none error=boom")),
            Some(id)
        );
        // The approval event carries no session id at all.
        assert_eq!(
            task_id(&format!("task_id={id} interaction_id={}", Uuid::new_v4())),
            Some(id)
        );
        assert_eq!(task_id("session_id=none"), None);
    }

    #[test]
    fn a_replayed_task_is_announced_once() {
        let mut seen = Seen {
            tasks: VecDeque::new(),
        };
        let task = Uuid::new_v4();
        assert!(seen.fresh(task));
        assert!(!seen.fresh(task));
        assert!(seen.fresh(Uuid::new_v4()));
    }

    #[test]
    fn an_open_tui_on_the_same_session_keeps_the_desktop_quiet() {
        let home = home();
        let session = Uuid::new_v4();
        let _presence = tui_presence::TuiPresence::acquire(&home, session).unwrap();
        let mut seen = Seen {
            tasks: VecDeque::new(),
        };
        let task = Uuid::new_v4();
        let decision = decide(
            &home,
            Language::En,
            &mut seen,
            &event("task.completed", task),
            Some(session),
        );
        if cfg!(windows) {
            assert!(matches!(decision, Decision::Announce(_)));
        } else {
            assert_eq!(decision, Decision::Quiet);
        }
    }

    #[test]
    fn a_session_without_a_tui_is_announced_without_the_error_text() {
        let home = home();
        let mut seen = Seen {
            tasks: VecDeque::new(),
        };
        let decision = decide(
            &home,
            Language::ZhCn,
            &mut seen,
            &event("task.waiting_approval", Uuid::new_v4()),
            None,
        );
        let Decision::Announce(notice) = decision else {
            panic!("expected an announcement");
        };
        let line = serde_json::to_string(&notice).unwrap();
        assert!(line.contains("有一项操作等你确认"));
        assert!(!line.contains("secret"), "{line}");
        assert!(!line.contains("error"), "{line}");
    }

    #[test]
    fn a_long_title_is_cut_on_a_character_boundary() {
        let title = "甲".repeat(TITLE_LIMIT + 5);
        let clamped = clamp(&title);
        assert!(clamped.ends_with('…'));
        assert_eq!(clamped.chars().count(), TITLE_LIMIT + 1);
    }

    #[test]
    fn an_event_the_shell_does_not_care_about_stays_quiet() {
        let home = home();
        let mut seen = Seen {
            tasks: VecDeque::new(),
        };
        let decision = decide(
            &home,
            Language::Ja,
            &mut seen,
            &event("turn.completed", Uuid::new_v4()),
            None,
        );
        assert_eq!(decision, Decision::Quiet);
    }
}
