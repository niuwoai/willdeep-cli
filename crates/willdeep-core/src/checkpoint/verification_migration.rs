//! Recover only invocation errors proven by original foreground tool round trips.

use std::collections::HashMap;

use super::VerificationEvidence;
use crate::tools::{ToolRegistry, VerificationStatus};
use crate::types::{Message, Role};

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Invocation {
    command: String,
    cwd: Option<String>,
    run_in_background: Option<bool>,
    #[serde(rename = "timeout_seconds")]
    _timeout_seconds: Option<u64>,
    #[serde(rename = "label")]
    _label: Option<String>,
    #[serde(rename = "network")]
    _network: Option<bool>,
}

/// Old tool output has no snapshot binding. Only an intact native history can
/// prove that every attempt of a command was rejected before running tests.
/// Counts alone cannot account for failures removed by materialized compression.
pub(super) fn migrate(
    mut evidence: Vec<VerificationEvidence>,
    session: &crate::session::Session,
) -> Vec<VerificationEvidence> {
    if session.compression_generation != 0
        || session.compression_checkpoint.is_some()
        || session.swift_source.is_some()
        || session.messages.iter().any(|message| {
            message.role == Role::Assistant && message.content.starts_with("<context-summary ")
        })
    {
        return evidence;
    }
    // Automatic summaries are request projections; unlike manual compression,
    // they leave these durable messages intact and do not prevent migration.
    let messages = &session.messages;
    let mut call_counts = HashMap::<&str, usize>::new();
    let mut results = HashMap::<&str, Option<(usize, &Message)>>::new();
    for (index, message) in messages.iter().enumerate() {
        // Count IDs across all roles/tools: a duplicate cannot establish identity.
        for call in &message.tool_calls {
            *call_counts.entry(&call.id).or_default() += 1;
        }
        if let Some(id) = message.tool_call_id.as_deref() {
            results
                .entry(id)
                .and_modify(|result| *result = None)
                .or_insert(Some((index, message)));
        }
    }
    let mut attempts = HashMap::<String, Option<usize>>::new();
    for (index, message) in messages.iter().enumerate() {
        if message.role != Role::Assistant {
            continue;
        }
        for call in &message.tool_calls {
            if call.name != "run_command" {
                continue;
            }
            let Ok(args) = serde_json::from_str::<Invocation>(&call.arguments) else {
                // An unparseable invocation cannot safely be assigned to a command.
                return evidence;
            };
            let command = args.command.trim();
            let count = attempts.entry(command.to_owned()).or_insert(Some(0));
            let valid = !call.id.trim().is_empty()
                && call_counts.get(call.id.as_str()) == Some(&1)
                && matches!(args.cwd.as_deref(), None | Some("."))
                && !args.run_in_background.unwrap_or(false)
                && results
                    .get(call.id.as_str())
                    .and_then(|result| *result)
                    .is_some_and(|(result_index, result)| {
                        result_index > index
                            && result.role == Role::Tool
                            && result.tool_calls.is_empty()
                            && is_invocation_error(command, &result.content)
                    });
            if valid {
                if let Some(count) = count {
                    *count += 1;
                }
            } else {
                *count = None;
            }
        }
    }
    let mut failed_counts = HashMap::<String, usize>::new();
    for record in &evidence {
        if record.status == VerificationStatus::Failed {
            *failed_counts
                .entry(record.command.trim().to_owned())
                .or_default() += 1;
        }
    }
    for record in &mut evidence {
        let command = record.command.trim();
        if record.status == VerificationStatus::Failed
            && let Some(Some(count)) = attempts.get(command)
            && failed_counts
                .get(command)
                .is_some_and(|failed| failed <= count)
        {
            record.status = VerificationStatus::InvocationError;
        }
    }
    evidence
}

fn is_invocation_error(command: &str, output: &str) -> bool {
    let Some((code, body)) = output
        .strip_prefix("exit_code: ")
        .and_then(|body| body.split_once('\n'))
    else {
        return false;
    };
    let Ok(code_value) = code.parse::<i32>() else {
        return false;
    };
    // Cargo's package-selection error emits no stdout. Require the original
    // run_command envelope, not a JSON fragment, notice, or quoted diagnostic.
    code == code_value.to_string()
        && body.starts_with("stdout:\n\nstderr:\n")
        && ToolRegistry::is_verification_invocation_error(command, Some(code_value), output)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::checkpoint::{
        CheckpointMetadata, CheckpointRecorder, CheckpointSink, CheckpointStatus,
        SessionCheckpointSink,
    };
    use crate::types::ToolCall;
    use crate::{ApprovalMode, Session, SessionStore};

    const COMMAND: &str = "cargo test -p willdeep-cli";
    const ERROR: &str = "exit_code: 101\nstdout:\n\nstderr:\nerror: package ID specification `willdeep-cli` did not match any packages\n\nhelp: a package with a similar name exists: `willdeep`\n";

    fn migrate(
        evidence: Vec<VerificationEvidence>,
        messages: &[Message],
    ) -> Vec<VerificationEvidence> {
        let mut session = Session::new(std::path::PathBuf::new(), None, "migration fixture");
        session.messages = messages.to_vec();
        super::migrate(evidence, &session)
    }

    #[test]
    fn compressed_or_imported_history_cannot_reclassify_missing_real_failures() {
        let evidence = vec![
            failed(COMMAND, "real-failure"),
            failed(COMMAND, "package-renamed"),
        ];
        let mut session = Session::new(std::path::PathBuf::new(), None, "work");
        // The original real failure was removed; two newer selector errors must
        // not be assigned to the two snapshots merely because counts match.
        session.messages = error_pair("new-1");
        session.messages.extend(error_pair("new-2"));
        session.compression_generation = 1;
        assert_eq!(super::migrate(evidence.clone(), &session), evidence);
        session.compression_generation = 0;
        session.compression_checkpoint = Some(crate::session::CompressionCheckpoint {
            generation: 1,
            previous_message_count: 10,
            compressed_message_count: 4,
            created_at: 0,
        });
        assert_eq!(super::migrate(evidence.clone(), &session), evidence);
        session.compression_checkpoint = None;
        session.swift_source = Some(std::path::PathBuf::from("imported.json"));
        assert_eq!(super::migrate(evidence.clone(), &session), evidence);
        session.swift_source = None;
        session.messages.insert(
            0,
            Message::assistant(
                "<context-summary source=\"derived-untrusted-history\">old work</context-summary>",
                vec![],
            ),
        );
        assert_eq!(super::migrate(evidence.clone(), &session), evidence);
    }

    fn failed(command: &str, snapshot: &str) -> VerificationEvidence {
        VerificationEvidence {
            snapshot_id: Some(snapshot.into()),
            command: command.into(),
            status: VerificationStatus::Failed,
        }
    }

    fn pair(id: &str, arguments: serde_json::Value, output: &str) -> Vec<Message> {
        let call = ToolCall {
            id: id.into(),
            name: "run_command".into(),
            arguments: arguments.to_string(),
        };
        vec![
            Message::assistant("", vec![call.clone()]),
            Message::tool(&call, output),
        ]
    }

    fn error_pair(id: &str) -> Vec<Message> {
        pair(id, serde_json::json!({ "command": COMMAND }), ERROR)
    }

    fn assert_unchanged(messages: &[Message]) {
        let original = vec![failed(COMMAND, "old")];
        assert_eq!(migrate(original.clone(), messages), original);
    }

    #[test]
    fn legacy_package_error_migrates_only_failed_evidence() {
        for cwd in [None, Some(".")] {
            let mut args = serde_json::json!({"command": COMMAND, "run_in_background": false});
            args["cwd"] = serde_json::json!(cwd);
            let mut passed = failed(COMMAND, "passed");
            passed.status = VerificationStatus::Passed;
            let original = vec![failed(COMMAND, "old"), failed("cargo test", "real"), passed];
            let migrated = migrate(original.clone(), &pair("a", args, ERROR));
            assert_eq!(migrated[0].status, VerificationStatus::InvocationError);
            assert_eq!(migrated[0].command, COMMAND);
            assert_eq!(migrated[0].snapshot_id, original[0].snapshot_id);
            assert_eq!(migrated[1..], original[1..]);
            assert_eq!(migrate(migrated.clone(), &[]), migrated);
        }
    }

    #[test]
    fn legacy_multiline_cargo_warning_preserves_diagnostic_requirements() {
        let warning = "warning: profiles for the non root package will be ignored, specify profiles at the workspace root:\npackage:   /repo/member/Cargo.toml\nworkspace: /repo/Cargo.toml\n";
        let output = ERROR.replacen("stderr:\n", &format!("stderr:\n{warning}"), 1);
        let original = vec![failed(COMMAND, "old")];
        let messages = pair(
            "native-warning",
            serde_json::json!({"command": COMMAND}),
            &output,
        );
        let migrated = migrate(original.clone(), &messages);
        assert_eq!(migrated[0].status, VerificationStatus::InvocationError);
        assert_eq!(migrated[0].snapshot_id, original[0].snapshot_id);

        for untrusted in [
            output.replace("workspace: /repo/Cargo.toml\n", ""),
            format!("{output}error[E0308]: mismatched types\n"),
            format!("{output}test result: FAILED. 0 passed; 1 failed\n"),
        ] {
            assert_unchanged(&pair(
                "mixed-failure",
                serde_json::json!({"command": COMMAND}),
                &untrusted,
            ));
        }
    }

    #[test]
    fn missing_or_out_of_order_results_keep_failures() {
        let messages = error_pair("a");
        assert_unchanged(&[]);
        assert_unchanged(&messages[..1]);
        assert_unchanged(&messages[1..]);
        assert_unchanged(&[messages[1].clone(), messages[0].clone()]);
        let mut unmatched = messages.clone();
        unmatched[1].tool_call_id = Some("other".into());
        assert_unchanged(&unmatched);
        let mut partial = error_pair("complete");
        partial.push(messages[0].clone());
        assert_unchanged(&partial);
    }

    #[test]
    fn prose_and_wrong_roles_do_not_establish_tool_evidence() {
        for role in [Role::User, Role::System, Role::Tool] {
            let mut messages = error_pair("a");
            messages[0].role = role;
            assert_unchanged(&messages);
        }
        for role in [Role::User, Role::System, Role::Assistant] {
            let mut messages = error_pair("a");
            messages[1].role = role;
            assert_unchanged(&messages);
        }
        assert_unchanged(&[
            Message::user(ERROR),
            Message::assistant(format!("{COMMAND}\n{ERROR}"), vec![]),
            Message::system(ERROR),
        ]);
        let mut messages = error_pair("a");
        messages[0].tool_calls[0].name = "other_tool".into();
        assert_unchanged(&messages);
    }

    #[test]
    fn non_root_background_and_malformed_arguments_keep_failures() {
        for extra in [
            serde_json::json!({"cwd": "crates/willdeep-cli"}),
            serde_json::json!({"cwd": "./"}),
            serde_json::json!({"cwd": ""}),
            serde_json::json!({"run_in_background": true}),
            serde_json::json!({"run_in_background": "false"}),
            serde_json::json!({"timeout_seconds": "60"}),
            serde_json::json!({"cwd": 1}),
        ] {
            let mut args = serde_json::json!({"command": COMMAND});
            args.as_object_mut()
                .unwrap()
                .extend(extra.as_object().unwrap().clone());
            let mut messages = error_pair("good");
            messages.extend(pair("bad", args, ERROR));
            assert_unchanged(&messages);
        }
        for malformed in ["{", "[]", "{}", "{\"command\":1}"] {
            let mut messages = error_pair("a");
            messages[0].tool_calls[0].arguments = malformed.into();
            messages.extend(error_pair("b"));
            assert_unchanged(&messages);
        }
    }

    #[test]
    fn duplicate_or_empty_ids_keep_failures() {
        let mut duplicate_calls = error_pair("a");
        duplicate_calls.push(duplicate_calls[0].clone());
        assert_unchanged(&duplicate_calls);
        let mut duplicate_results = error_pair("a");
        duplicate_results.push(duplicate_results[1].clone());
        assert_unchanged(&duplicate_results);
        let mut reused = error_pair("a");
        reused.extend(error_pair("a"));
        assert_unchanged(&reused);
        assert_unchanged(&error_pair(""));
    }

    #[test]
    fn any_real_failure_pass_or_unknown_result_blocks_the_command() {
        for output in [
            "exit_code: 101\nstdout:\n\nstderr:\nerror: test failed, to rerun pass `-p willdeep-cli --lib`\n",
            "exit_code: 0\nstdout:\ntest result: ok. 1 passed\nstderr:\n",
            "exit_code: unknown\nstdout:\n\nstderr:\n",
            "command timed out",
            ERROR.trim_start_matches("exit_code: 101\n"),
            "exit_code: 101\nstdout:\n\nstderr:\n",
            "exit_code: 101\nstdout:\nquoted diagnostic\nstderr:\nerror: package ID specification `willdeep-cli` did not match any packages\n",
            "exit_code: 101\nstdout:\n\nstderr:\nerror: package ID specification `other` did not match any packages\n",
        ] {
            let mut messages = error_pair("error");
            messages.extend(pair(
                "other",
                serde_json::json!({"command": COMMAND}),
                output,
            ));
            assert_unchanged(&messages);
        }
    }

    #[test]
    fn failed_snapshots_require_at_least_as_many_matching_invocations() {
        let evidence = vec![failed(COMMAND, "first"), failed(COMMAND, "second")];
        let mut messages = error_pair("a");
        assert_eq!(migrate(evidence.clone(), &messages), evidence);
        messages.extend(error_pair("b"));
        assert!(
            migrate(evidence, &messages)
                .iter()
                .all(|record| record.status == VerificationStatus::InvocationError)
        );
    }

    #[test]
    fn session_sink_migration_is_read_only_then_persists_on_normal_checkpoint() {
        let root =
            std::env::temp_dir().join(format!("verification-migration-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let store = SessionStore::new(root.join("state"));
        let mut session = Session::new(root.clone(), None, "work");
        session.messages = error_pair("a");
        session.execution_checkpoint = Some(CheckpointMetadata {
            status: CheckpointStatus::Partial,
            required_verifications: vec![COMMAND.into(), "cargo test".into()],
            verification_evidence: vec![failed(COMMAND, "old"), failed("cargo test", "real")],
            ..Default::default()
        });
        store.save(&mut session).unwrap();
        let sink = SessionCheckpointSink {
            store,
            session_id: session.id,
        };
        let recovered = sink.verification_evidence().unwrap();
        assert_eq!(recovered[0].status, VerificationStatus::InvocationError);
        assert_eq!(recovered[1].status, VerificationStatus::Failed);
        let on_disk = sink
            .store
            .load(session.id)
            .unwrap()
            .execution_checkpoint
            .unwrap();
        assert_eq!(
            on_disk.verification_evidence[0].status,
            VerificationStatus::Failed
        );
        assert_eq!(
            sink.required_verifications().unwrap(),
            on_disk.required_verifications
        );
        // An invocation error alone cannot establish a pass, even on its own snapshot.
        let invocation_only = ToolRegistry::new(&root, ApprovalMode::ReadOnly)
            .unwrap()
            .with_verification_snapshot(|| Some("old".into()));
        invocation_only.restore_verification_evidence(vec![recovered[0].clone()]);
        assert!(!invocation_only.completion_has_current_evidence(Some("old")));
        let tools = ToolRegistry::new(&root, ApprovalMode::ReadOnly)
            .unwrap()
            .with_verification_snapshot(|| Some("old".into()));
        let mut recorder = CheckpointRecorder::new(Some(&sink));
        recorder.initialize_evidence(&tools).unwrap();
        assert_eq!(tools.required_verifications(), vec!["cargo test", COMMAND]);
        assert!(!tools.completion_has_current_evidence(Some("old")));
        recorder.record(&session.messages, 1, 0, 0).unwrap();
        drop(recorder);
        let persisted = sink
            .store
            .load(session.id)
            .unwrap()
            .execution_checkpoint
            .unwrap();
        assert_eq!(persisted.verification_evidence.len(), recovered.len());
        assert!(
            persisted
                .verification_evidence
                .iter()
                .all(|record| recovered.contains(record))
        );
        assert_eq!(
            persisted.required_verifications,
            vec!["cargo test", COMMAND]
        );
        let restored = ToolRegistry::new(&root, ApprovalMode::ReadOnly)
            .unwrap()
            .with_verification_snapshot(|| Some("old".into()));
        CheckpointRecorder::new(Some(&sink))
            .initialize_evidence(&restored)
            .unwrap();
        let restored_evidence = restored.verification_evidence();
        assert_eq!(restored_evidence.len(), recovered.len());
        assert!(
            restored_evidence
                .iter()
                .all(|record| recovered.contains(record))
        );
        assert!(!restored.completion_has_current_evidence(Some("old")));
        assert_eq!(
            restored.required_verifications(),
            vec!["cargo test", COMMAND]
        );
        // Completed task evidence must never leak into a new executor.
        sink.store
            .update(session.id, |session| {
                session.execution_checkpoint.as_mut().unwrap().status = CheckpointStatus::Completed;
            })
            .unwrap();
        assert!(sink.verification_evidence().unwrap().is_empty());
        std::fs::remove_dir_all(root).unwrap();
    }
}
