//! Recognize only an explicit leading literal sleep. The shell program is never
//! rewritten, split, restarted, or treated as proof of a completed verification.
use std::sync::LazyLock;

const MIN_BACKGROUND_WAIT_SECONDS: u64 = 30;
const NOTICE_PREFIX: &str = "[background-delay seconds=";

static LEADING_SLEEP: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(
        r"^\s*(?:sleep|/bin/sleep|/usr/bin/sleep)[ \t]+([0-9]+)([smhd]?)[ \t]*(?:;|&&|\r?\n|$)",
    )
    .expect("literal leading sleep expression")
});

pub(super) fn leading_sleep_seconds(command: &str) -> Option<u64> {
    let captures = LEADING_SLEEP.captures(command)?;
    let value: u64 = captures[1].parse().ok()?;
    let multiplier = match &captures[2] {
        "m" => 60,
        "h" => 3600,
        "d" => 86400,
        _ => 1,
    };
    let seconds = value.checked_mul(multiplier)?;
    (seconds >= MIN_BACKGROUND_WAIT_SECONDS).then_some(seconds)
}

pub(super) fn timeout_seconds(explicit: Option<u64>, automatic_delay: Option<u64>) -> u64 {
    explicit
        .unwrap_or_else(|| {
            automatic_delay
                .unwrap_or(0)
                .saturating_add(super::DEFAULT_COMMAND_TIMEOUT_SECS)
        })
        .clamp(1, super::MAX_COMMAND_TIMEOUT_SECS)
}

pub(super) fn started_notice(started: String, delay: Option<u64>) -> String {
    match delay {
        Some(seconds) => format!(
            "{NOTICE_PREFIX}{seconds}]\n{started}\nThe entire original command is running once in the background, including its leading {seconds}-second wait. Handle new user messages while it waits. Do not rerun, sleep, or poll for this command; its completion will be delivered automatically."
        ),
        None => started,
    }
}

/// Numeric host metadata for frontends; no command text, credentials, or output
/// needs to be forwarded through the public Runtime event stream.
pub fn notice_wait_seconds(output: &str) -> Option<u64> {
    output
        .strip_prefix(NOTICE_PREFIX)?
        .split_once("]\nBackground ")?
        .0
        .parse()
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_literal_leading_long_waits_are_recognized() {
        for (command, seconds) in [
            ("sleep 240;cd project && check", 240),
            ("  /bin/sleep 30 && check", 30),
            ("/usr/bin/sleep 2m\ncheck", 120),
            ("sleep 1h", 3600),
        ] {
            assert_eq!(leading_sleep_seconds(command), Some(seconds), "{command}");
        }
        for command in [
            "sleep 1; check",
            "printf 'sleep 240; check'",
            "echo ok; sleep 240",
            "sleep $DELAY; check",
            "sleep $(get_delay); check",
            "sleep 30 & check",
            "sleep 30 | check",
            "sleep 30 || check",
            "sleep 240ms; check",
            "sleep 999999999999999999999999999; check",
        ] {
            assert_eq!(leading_sleep_seconds(command), None, "{command}");
        }
    }

    #[test]
    fn automatic_default_includes_wait_but_explicit_timeouts_remain_authoritative() {
        assert_eq!(timeout_seconds(None, Some(240)), 300);
        assert_eq!(timeout_seconds(Some(10), Some(240)), 10);
        assert_eq!(
            timeout_seconds(None, Some(u64::MAX)),
            super::super::MAX_COMMAND_TIMEOUT_SECS
        );
        assert_eq!(
            timeout_seconds(None, None),
            super::super::DEFAULT_COMMAND_TIMEOUT_SECS
        );
        let notice = started_notice("Background job started: job_fixture".into(), Some(240));
        assert_eq!(notice_wait_seconds(&notice), Some(240));
        assert_eq!(notice_wait_seconds("exit_code: 0\nstdout: sleep 240"), None);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn automatic_wait_returns_immediately_and_remains_cancellable() {
        use super::super::*;
        let root = std::env::temp_dir().join(format!("delayed-command-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let registry = ToolRegistry::new(&root, ApprovalMode::FullAccess).unwrap();
        let mut events = registry.background.subscribe();
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            registry.run_command(CommandArgs {
                cwd: None,
                command: "sleep 240; printf once >> should-not-exist".into(),
                timeout_seconds: None,
                label: Some("delayed check".into()),
                run_in_background: Some(false),
                network: None,
            }),
        )
        .await
        .expect("long waits must not block the caller")
        .unwrap();
        assert_eq!(notice_wait_seconds(&result), Some(240));
        assert!(!root.join("should-not-exist").exists());
        let tasks = registry.background.snapshots();
        assert_eq!(tasks.len(), 1);
        assert!(registry.background.kill(&tasks[0].id));
        let event = tokio::time::timeout(std::time::Duration::from_secs(3), events.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(event.snapshot.status, BackgroundTaskStatus::Killed);
        assert!(!root.join("should-not-exist").exists());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn automatic_wait_cannot_bypass_destructive_command_approval() {
        use super::super::*;
        let root =
            std::env::temp_dir().join(format!("delayed-command-gate-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let registry = ToolRegistry::new(&root, ApprovalMode::FullAccess).unwrap();
        let result = registry
            .run_command(CommandArgs {
                cwd: None,
                command: "sleep 240; rm -rf previous-results".into(),
                timeout_seconds: None,
                label: None,
                run_in_background: None,
                network: None,
            })
            .await;
        assert!(result.is_err());
        assert!(registry.background.snapshots().is_empty());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn scoped_worker_verifier_stays_foreground_until_exit_or_deadline() {
        use super::super::*;
        let root = std::env::temp_dir().join(format!("delayed-verifier-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let command = "sleep 30; printf should-not-run";
        let registry = ToolRegistry::new(&root, ApprovalMode::FullAccess)
            .unwrap()
            .with_command_allowlist(Some(HashSet::from([command.to_owned()])));
        registry.set_execution_deadline(Some(
            tokio::time::Instant::now() + std::time::Duration::from_millis(50),
        ));
        let result = registry
            .run_command(CommandArgs {
                cwd: None,
                command: command.into(),
                timeout_seconds: None,
                label: None,
                run_in_background: None,
                network: None,
            })
            .await;
        assert!(matches!(result, Err(ToolError::CommandTimeout { .. })));
        assert!(registry.background.snapshots().is_empty());
        std::fs::remove_dir_all(root).unwrap();
    }
}
