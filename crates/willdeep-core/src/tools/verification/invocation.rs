//! A rejected Cargo package selector is an unsuccessful attempt, not a test failure.
//! Keep recognition narrow: exit 101 alone also covers real compiler/test failures.
use super::*;

const CARGO_INVOCATION_EXIT: i32 = 101;
const CARGO_NON_ROOT_PROFILES_WARNING: &str = "warning: profiles for the non root package will be ignored, specify profiles at the workspace root:";

impl ToolRegistry {
    pub(crate) fn is_verification_invocation_error(
        command: &str,
        exit_code: Option<i32>,
        output: &str,
    ) -> bool {
        if exit_code != Some(CARGO_INVOCATION_EXIT) {
            return false;
        }
        let command = if command.starts_with("[cwd:") {
            let Some((_, command)) = command.split_once("] ") else {
                return false;
            };
            command
        } else {
            command
        };
        let Some((package, _)) = cargo_selection(command) else {
            return false;
        };
        // Foreground results include an exit-code envelope; background results contain
        // combined stdout/stderr. Nonempty stdout or an unfamiliar envelope fails closed.
        // Older persisted tool results can carry this exact host-generated suffix.
        // Match the complete known hint, never strip arbitrary XML or model prose.
        let hint = delegation_hint("test_fixer", command);
        let output = output.strip_suffix(&hint).unwrap_or(output);
        let diagnostic = output
            .strip_prefix("exit_code: 101\nstdout:\n\nstderr:\n")
            .unwrap_or(output);
        let Some(diagnostic) = without_cargo_colors(diagnostic) else {
            return false;
        };
        let expected =
            format!("error: package ID specification `{package}` did not match any packages");
        let mut lines = diagnostic.lines();
        while let Some(line) = lines.next() {
            if line == CARGO_NON_ROOT_PROFILES_WARNING {
                // This native warning has exactly two manifest-path rows. Never
                // skip arbitrary continuation lines or an incomplete warning.
                for prefix in ["package:   ", "workspace: "] {
                    let Some(manifest) = lines.next().and_then(|line| line.strip_prefix(prefix))
                    else {
                        return false;
                    };
                    if Path::new(manifest).file_name() != Some(std::ffi::OsStr::new("Cargo.toml")) {
                        return false;
                    }
                }
                continue;
            }
            if line.is_empty() || line.starts_with("warning: ") {
                continue;
            }
            return line == expected
                && lines.all(|line| line.is_empty() || line.starts_with("help: "));
        }
        false
    }
}

/// Cargo's explicit --color always uses SGR. Strip only those color sequences;
/// cursor movement, control strings and malformed escapes are not diagnostics.
fn without_cargo_colors(output: &str) -> Option<String> {
    let mut plain = String::with_capacity(output.len());
    let mut chars = output.chars();
    while let Some(ch) = chars.next() {
        if ch != '\u{1b}' {
            plain.push(ch);
            continue;
        }
        if chars.next() != Some('[') {
            return None;
        }
        loop {
            match chars.next()? {
                'm' => break,
                ch if ch.is_ascii_digit() || ch == ';' => {}
                _ => return None,
            }
        }
    }
    Some(plain)
}

/// A correction changes only one package selector. Keep cwd, test filters,
/// features and every other argument, so unrelated passing checks cannot retire it.
pub(super) fn corrects_invocation_error(rejected: &str, passed: &str) -> bool {
    match (cargo_selection(rejected), cargo_selection(passed)) {
        (Some((old_package, old_key)), Some((new_package, new_key))) => {
            old_package != new_package && old_key == new_key
        }
        _ => false,
    }
}

fn cargo_selection(command: &str) -> Option<(String, Vec<String>)> {
    let (scope, command) = if command.starts_with("[cwd:") {
        let (scope, command) = command.split_once("] ")?;
        (scope, command)
    } else {
        ("", command)
    };
    let words = verification_words(command)?;
    if !words
        .iter()
        .take(2)
        .map(String::as_str)
        .eq(["cargo", "test"])
    {
        return None;
    }
    let mut key = vec![scope.to_owned(), "cargo".into(), "test".into()];
    let mut package = None;
    let mut arguments = words.iter().skip(2);
    while let Some(argument) = arguments.next() {
        if argument == "--" {
            key.push(argument.clone());
            key.extend(arguments.cloned());
            break;
        }
        let selector = match argument.as_str() {
            "-p" | "--package" => Some(arguments.next()?.as_str()),
            value if value.starts_with("--package=") => value.strip_prefix("--package="),
            value if value.starts_with("-p") => value.strip_prefix("-p"),
            _ => None,
        };
        if let Some(selector) = selector {
            if package.is_some() || !valid_target_name(selector) {
                return None;
            }
            package = Some(selector.to_owned());
            key.push("--package".into());
            key.push("<corrected-package>".into());
        } else {
            key.push(argument.clone());
        }
    }
    Some((package?, key))
}

#[cfg(test)]
mod tests {
    use super::*;

    const BAD: &str = "cargo test -p willdeep-cli tui::tests::viewport";
    const GOOD: &str = "cargo test -p willdeep tui::tests::viewport";
    const DIAGNOSTIC: &str = "error: package ID specification `willdeep-cli` did not match any packages\n\nhelp: a package with a similar name exists: `willdeep-core`\n";

    fn registry() -> (ToolRegistry, PathBuf) {
        let root =
            std::env::temp_dir().join(format!("willdeep-invocation-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let tools = ToolRegistry::new(&root, ApprovalMode::ReadOnly)
            .unwrap()
            .with_verification_snapshot(|| Some("current".into()))
            .with_verification_reporter(|_| {});
        (tools, root)
    }

    fn attempt(tools: &ToolRegistry, command: &str, status: VerificationStatus, output: &str) {
        report_verification(
            tools.verification_reporter.as_ref(),
            command,
            Some(if status == VerificationStatus::Passed {
                0
            } else {
                101
            }),
            status,
            output,
            Some("current".into()),
        );
    }

    #[test]
    fn cargo_package_invocation_requires_matching_command_exit_and_diagnostic() {
        for command in [
            BAD,
            "cargo test --package=willdeep-cli",
            "cargo test -pwilldeep-cli",
        ] {
            assert!(ToolRegistry::is_verification_invocation_error(
                command,
                Some(101),
                DIAGNOSTIC
            ));
        }
        let foreground = format!("exit_code: 101\nstdout:\n\nstderr:\n{DIAGNOSTIC}");
        assert!(ToolRegistry::is_verification_invocation_error(
            BAD,
            Some(101),
            &foreground
        ));
        assert!(ToolRegistry::is_verification_invocation_error(
            BAD,
            Some(101),
            &format!("{foreground}{}", delegation_hint("test_fixer", BAD))
        ));
        assert!(!ToolRegistry::is_verification_invocation_error(
            BAD,
            Some(101),
            &format!("{foreground}\n<delegation-hint>unknown</delegation-hint>")
        ));
        for command in [
            GOOD,
            "cargo test",
            "cargo test -- -p willdeep-cli",
            "cargo test -p willdeep-cli -p willdeep-core",
            "cargo test -p willdeep-cli | tail -1",
            "npm test -p willdeep-cli",
        ] {
            assert!(
                !ToolRegistry::is_verification_invocation_error(command, Some(101), DIAGNOSTIC),
                "{command}"
            );
        }
        for (code, output) in [
            (Some(0), DIAGNOSTIC),
            (Some(1), DIAGNOSTIC),
            (None, DIAGNOSTIC),
            (
                Some(101),
                "error: could not compile `willdeep-cli` (lib test)",
            ),
            (Some(101), "test result: FAILED. 1 failed"),
            (Some(101), "unexpected error"),
        ] {
            assert!(!ToolRegistry::is_verification_invocation_error(
                BAD, code, output
            ));
        }
        assert!(!ToolRegistry::is_verification_invocation_error(
            BAD,
            Some(101),
            &format!("{DIAGNOSTIC}test result: FAILED\n")
        ));
        assert!(!ToolRegistry::is_verification_invocation_error(
            BAD,
            Some(101),
            &format!("exit_code: 101\nstdout:\ntests failed\nstderr:\n{DIAGNOSTIC}")
        ));
    }

    #[test]
    fn cargo_color_and_manifest_warnings_do_not_create_permanent_failures() {
        let command = "cargo test --color always -p willdeep-cli tui::tests::viewport";
        let colored = DIAGNOSTIC.replace(
            "error:",
            "\u{1b}[1m\u{1b}[31merror\u{1b}[0m\u{1b}[1m:\u{1b}[0m",
        );
        for diagnostic in [
            colored,
            format!("warning: unused manifest key: package.unknown\n{DIAGNOSTIC}"),
        ] {
            let output = format!("exit_code: 101\nstdout:\n\nstderr:\n{diagnostic}");
            let (tools, root) = registry();
            attempt(&tools, command, VerificationStatus::Failed, &output);
            assert_eq!(tools.verification_count(), 0);
            attempt(
                &tools,
                "cargo test --color always -p willdeep tui::tests::viewport",
                VerificationStatus::Passed,
                "ok",
            );
            assert!(tools.completion_has_current_evidence(Some("current")));
            std::fs::remove_dir_all(root).unwrap();
        }
        for prefix in [
            "\u{1b}[2K",
            "\u{1b}]8;;example\u{7}",
            "\u{1b}[31",
            "test result: FAILED\n",
        ] {
            assert!(!ToolRegistry::is_verification_invocation_error(
                command,
                Some(101),
                &format!("{prefix}{DIAGNOSTIC}")
            ));
        }
    }

    #[test]
    fn cargo_non_root_profiles_warning_accepts_only_complete_native_blocks() {
        let warning = format!(
            "{CARGO_NON_ROOT_PROFILES_WARNING}\npackage:   /workspace/member/Cargo.toml\nworkspace: /workspace/Cargo.toml\n"
        );
        for prefix in [
            warning.clone(),
            format!("warning: unused manifest key: package.unknown\n{warning}"),
            format!("{warning}\n{warning}"),
            warning.replace("warning:", "\u{1b}[1m\u{1b}[33mwarning\u{1b}[0m:"),
        ] {
            for envelope in ["", "exit_code: 101\nstdout:\n\nstderr:\n"] {
                let output = format!("{envelope}{prefix}{DIAGNOSTIC}");
                assert!(
                    ToolRegistry::is_verification_invocation_error(BAD, Some(101), &output),
                    "{output}"
                );
            }
        }
    }

    #[test]
    fn cargo_non_root_profiles_warning_rejects_malformed_blocks_and_real_failures() {
        let warning = format!(
            "{CARGO_NON_ROOT_PROFILES_WARNING}\npackage:   /workspace/member/Cargo.toml\nworkspace: /workspace/Cargo.toml\n"
        );
        for prefix in [
            "package:   /workspace/member/Cargo.toml\nworkspace: /workspace/Cargo.toml\n".into(),
            format!("{CARGO_NON_ROOT_PROFILES_WARNING}\n"),
            format!("{CARGO_NON_ROOT_PROFILES_WARNING}\npackage:   /workspace/member/Cargo.toml\n"),
            format!(
                "{CARGO_NON_ROOT_PROFILES_WARNING}\nworkspace: /workspace/Cargo.toml\npackage:   /workspace/member/Cargo.toml\n"
            ),
            warning.replace("package:   ", "package: "),
            warning.replace("workspace: ", "workspace root: "),
            warning.replace("/workspace/member/Cargo.toml", ""),
            warning.replace("/workspace/Cargo.toml", ""),
            warning.replace(
                "/workspace/member/Cargo.toml",
                "error: could not compile `actual-package` (lib test)",
            ),
            warning.replace("/workspace/Cargo.toml", "test result: FAILED. 1 failed"),
            warning.replace("root:\n", "root: extra\n"),
            format!("unexpected output\n{warning}"),
            format!(
                "{CARGO_NON_ROOT_PROFILES_WARNING}\nerror: could not compile `actual-package` (lib test)\n{warning}"
            ),
            format!("{warning}error: could not compile `actual-package` (lib test)\n"),
            format!("{warning}test result: FAILED. 1 failed\n"),
        ] {
            let output = format!("{prefix}{DIAGNOSTIC}");
            assert!(
                !ToolRegistry::is_verification_invocation_error(BAD, Some(101), &output),
                "{output}"
            );
        }
        for failure in [
            "error: could not compile `actual-package` (lib test)",
            "test result: FAILED. 1 failed",
        ] {
            for output in [
                format!("{warning}{failure}\n"),
                format!("{warning}{DIAGNOSTIC}{failure}\n"),
                format!("{DIAGNOSTIC}{warning}{failure}\n"),
            ] {
                assert!(
                    !ToolRegistry::is_verification_invocation_error(BAD, Some(101), &output),
                    "{output}"
                );
            }
        }
    }

    #[test]
    fn corrected_package_check_clears_gate_without_erasing_attempt_history() {
        let (tools, root) = registry();
        attempt(&tools, BAD, VerificationStatus::Failed, DIAGNOSTIC);
        assert_eq!(tools.verification_count(), 0);
        assert_eq!(
            tools.run_verification(Some("current"), 0),
            (RunVerification::Unverified, 0)
        );
        assert!(!tools.completion_has_current_evidence(Some("current")));
        assert_eq!(
            tools.verification_records.lock().unwrap().recent[0].status,
            VerificationStatus::InvocationError
        );
        attempt(&tools, GOOD, VerificationStatus::Passed, "test result: ok");
        assert!(tools.completion_has_current_evidence(Some("current")));
        assert_eq!(
            tools.run_verification(Some("current"), 0),
            (RunVerification::Passed, 1)
        );
        let encoded = serde_json::to_vec(&tools.verification_evidence()).unwrap();
        let (restored, restored_root) = registry();
        restored.restore_verification_evidence(serde_json::from_slice(&encoded).unwrap());
        assert!(restored.completion_has_current_evidence(Some("older")));
        assert!(
            restored
                .verification_evidence()
                .iter()
                .any(|item| item.status == VerificationStatus::InvocationError)
        );
        std::fs::remove_dir_all(root).unwrap();
        std::fs::remove_dir_all(restored_root).unwrap();
    }

    #[test]
    fn invocation_errors_neither_replace_required_checks_nor_hide_real_failures() {
        let (tools, root) = registry();
        tools.require_verifications(&[BAD.into()]).unwrap();
        attempt(&tools, BAD, VerificationStatus::Failed, DIAGNOSTIC);
        attempt(&tools, GOOD, VerificationStatus::Passed, "ok");
        let feedback = tools
            .completion_verification_feedback(Some("current"))
            .unwrap();
        assert!(feedback.contains("invocation_error"));
        assert!(!tools.completion_has_current_evidence(Some("current")));
        std::fs::remove_dir_all(root).unwrap();

        let (tools, root) = registry();
        attempt(
            &tools,
            BAD,
            VerificationStatus::Failed,
            "test result: FAILED",
        );
        attempt(&tools, BAD, VerificationStatus::Failed, DIAGNOSTIC);
        attempt(&tools, GOOD, VerificationStatus::Passed, "ok");
        assert_eq!(
            tools.run_verification(Some("current"), 0).0,
            RunVerification::Failed
        );
        let feedback = tools
            .completion_verification_feedback(Some("current"))
            .unwrap();
        assert!(feedback.contains("\"status\":\"failed\""));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn correction_preserves_test_scope_and_requires_a_current_stable_pass() {
        let (tools, root) = registry();
        attempt(&tools, BAD, VerificationStatus::Failed, DIAGNOSTIC);
        for unrelated in [
            "cargo test unrelated",
            "cargo test -p willdeep tui::tests::other",
            "cargo test -p willdeep --features extra tui::tests::viewport",
        ] {
            attempt(&tools, unrelated, VerificationStatus::Passed, "ok");
            assert!(!tools.completion_has_current_evidence(Some("current")));
        }
        report_accepted_verification(
            tools.verification_reporter.as_ref(),
            "[cwd:child] cargo test -p willdeep tui::tests::viewport",
            Some(0),
            VerificationStatus::Passed,
            "ok",
            Some("current".into()),
            None,
        );
        assert!(!tools.completion_has_current_evidence(Some("current")));
        attempt(
            &tools,
            GOOD,
            VerificationStatus::Invalidated,
            "changed while running",
        );
        assert!(!tools.completion_has_current_evidence(Some("current")));
        let feedback = tools
            .completion_verification_feedback(Some("current"))
            .unwrap();
        assert!(feedback.contains("correct only the invalid package selector"));
        attempt(
            &tools,
            "cargo test --package=willdeep tui::tests::viewport",
            VerificationStatus::Passed,
            "ok",
        );
        // The separate invalidated exact command still needs its own stable rerun.
        assert!(!tools.completion_has_current_evidence(Some("current")));
        attempt(&tools, GOOD, VerificationStatus::Passed, "ok");
        assert!(tools.completion_has_current_evidence(Some("current")));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn restored_real_failure_outranks_a_live_invocation_error() {
        let (tools, root) = registry();
        attempt(&tools, BAD, VerificationStatus::Failed, DIAGNOSTIC);
        tools.restore_verification_evidence(vec![crate::checkpoint::VerificationEvidence {
            command: BAD.into(),
            snapshot_id: Some("current".into()),
            status: VerificationStatus::Failed,
        }]);
        attempt(&tools, GOOD, VerificationStatus::Passed, "ok");
        assert_eq!(
            tools.run_verification(Some("current"), 0).0,
            RunVerification::Failed
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn real_cargo_package_correction_works_in_foreground_and_background() {
        for (non_root_profile, in_background) in
            [(false, false), (false, true), (true, false), (true, true)]
        {
            let (_, root) = registry();
            let root = root.canonicalize().unwrap();
            let (package_root, manifest_suffix) = if non_root_profile {
                std::fs::write(
                    root.join("Cargo.toml"),
                    "[workspace]\nmembers = [\"member\"]\nresolver = \"2\"\n",
                )
                .unwrap();
                (root.join("member"), "[profile.dev]\nopt-level = 1\n")
            } else {
                (root.clone(), "[workspace]\n")
            };
            std::fs::create_dir_all(package_root.join("src")).unwrap();
            std::fs::write(
                package_root.join("Cargo.toml"),
                format!("[package]\nname = \"actual-package\"\nversion = \"0.1.0\"\nedition = \"2024\"\n{manifest_suffix}"),
            )
            .unwrap();
            std::fs::write(
                package_root.join("src/lib.rs"),
                "#[test] fn works() { assert_eq!(2 + 2, 4); }\n",
            )
            .unwrap();
            let background = Arc::new(BackgroundTaskRegistry::default());
            let mut events = background.subscribe();
            let tools = ToolRegistry::new(&root, ApprovalMode::FullAccess)
                .unwrap()
                .with_verification_snapshot(|| Some("current".into()))
                .with_verification_reporter(|_| {})
                .with_background_tasks(Arc::clone(&background));
            for (package, expected) in [
                ("missing-package", VerificationStatus::InvocationError),
                ("actual-package", VerificationStatus::Passed),
            ] {
                let call = ToolCall {
                    id: format!("cargo-{package}"),
                    name: "run_command".into(),
                    arguments: serde_json::json!({
                        "command": format!("cargo test --offline --color always --target-dir target -p {package}"),
                        "timeout_seconds": 30,
                        "run_in_background": in_background,
                    })
                    .to_string(),
                };
                let output = tools.execute(&call).await.unwrap();
                let diagnostic = if in_background {
                    let event =
                        tokio::time::timeout(std::time::Duration::from_secs(30), events.recv())
                            .await
                            .unwrap()
                            .unwrap();
                    assert_eq!(
                        event.snapshot.status,
                        if expected == VerificationStatus::Passed {
                            BackgroundTaskStatus::Completed
                        } else {
                            BackgroundTaskStatus::Failed
                        }
                    );
                    background.full_output(&event.snapshot.id).unwrap()
                } else {
                    assert!(
                        output.starts_with(if expected == VerificationStatus::Passed {
                            "exit_code: 0"
                        } else {
                            "exit_code: 101"
                        }),
                        "{output}"
                    );
                    output
                };
                if non_root_profile {
                    let plain = without_cargo_colors(&diagnostic).unwrap();
                    let warning = format!(
                        "{CARGO_NON_ROOT_PROFILES_WARNING}\npackage:   {}\nworkspace: {}\n",
                        package_root.join("Cargo.toml").display(),
                        root.join("Cargo.toml").display(),
                    );
                    assert!(plain.contains(&warning), "{diagnostic}");
                }
                assert_eq!(
                    tools
                        .verification_records
                        .lock()
                        .unwrap()
                        .recent
                        .last()
                        .unwrap()
                        .status,
                    expected,
                    "background: {in_background}, non-root profile: {non_root_profile}\n{diagnostic}"
                );
                assert_eq!(
                    tools.completion_has_current_evidence(Some("current")),
                    expected == VerificationStatus::Passed
                );
            }
            assert_eq!(
                tools.run_verification(Some("current"), 0),
                (RunVerification::Passed, 1)
            );
            std::fs::remove_dir_all(root).unwrap();
        }
    }

    #[test]
    fn unknown_legacy_failures_and_changed_snapshots_still_block() {
        let (tools, root) = registry();
        tools.restore_verification_evidence(vec![crate::checkpoint::VerificationEvidence {
            command: BAD.into(),
            snapshot_id: Some("older".into()),
            status: VerificationStatus::Failed,
        }]);
        attempt(&tools, BAD, VerificationStatus::Failed, DIAGNOSTIC);
        attempt(&tools, GOOD, VerificationStatus::Passed, "ok");
        assert!(!tools.completion_has_current_evidence(Some("older")));
        std::fs::remove_dir_all(root).unwrap();

        let (tools, root) = registry();
        tools.restore_verification_evidence(vec![
            crate::checkpoint::VerificationEvidence {
                command: BAD.into(),
                snapshot_id: Some("older".into()),
                status: VerificationStatus::InvocationError,
            },
            crate::checkpoint::VerificationEvidence {
                command: GOOD.into(),
                snapshot_id: Some("older".into()),
                status: VerificationStatus::Passed,
            },
        ]);
        assert!(!tools.completion_has_current_evidence(Some("older")));
        std::fs::remove_dir_all(root).unwrap();
    }
}
