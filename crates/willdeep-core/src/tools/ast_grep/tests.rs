use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use super::snapshot::{Language, Snapshot, collect};
use super::*;
use crate::tools::ApprovalMode;

struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        let path =
            std::env::temp_dir().join(format!("willdeep-ast-tests-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&path).unwrap();
        Self(path.canonicalize().unwrap())
    }
    fn write(&self, name: &str, text: impl AsRef<[u8]>) -> PathBuf {
        let path = self.0.join(name);
        std::fs::write(&path, text).unwrap();
        path
    }
    #[cfg(unix)]
    fn program(&self, name: &str, body: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let path = self.write(name, format!("#!/bin/sh\nset -eu\n{body}\n"));
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
        path
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        if let Err(error) = std::fs::remove_dir_all(&self.0) {
            eprintln!("remove isolated ast-grep test fixture: {error}");
        }
    }
}

fn arguments(value: Value) -> Arguments {
    Arguments::parse(value).unwrap()
}

#[cfg(unix)]
fn query_args() -> Arguments {
    arguments(json!({"operation":"query","path":"main.rs","language":"rust","pattern":"foo($A)"}))
}

#[test]
fn ast_arguments_reject_extra_execution_configuration_and_invalid_limits() {
    for key in [
        "command",
        "args",
        "config",
        "rewrite",
        "parser",
        "customLanguages",
        "install",
        "stdin",
        "rule",
    ] {
        let mut value = json!({"operation":"query","language":"rust","pattern":"foo($A)"});
        value[key] = json!("malicious");
        assert!(Arguments::parse(value).is_err(), "accepted {key}");
    }
    for value in [
        json!({"operation":"scan"}),
        json!({"operation":"status","path":"."}),
        json!({"operation":"status","pattern":"x"}),
        json!({"operation":"query","language":"javascript","pattern":"x"}),
        json!({"operation":"query","language":"rust","pattern":""}),
        json!({"operation":"query","language":"rust","pattern":"\u{0}"}),
        json!({"operation":"query","language":"rust","pattern":"x","limit":0}),
        json!({"operation":"query","language":"rust","pattern":"x","limit":201}),
        json!({"operation":"query","language":"rust","pattern":"x","timeout_seconds":0}),
        json!({"operation":"query","language":"rust","pattern":"x","timeout_seconds":61}),
        json!({"operation":"query","language":"rust","pattern":"x".repeat(MAX_PATTERN_BYTES + 1)}),
    ] {
        assert!(Arguments::parse(value.clone()).is_err(), "accepted {value}");
    }
}

#[test]
fn ast_arguments_accept_exact_public_builtin_language_names() {
    for (name, expected) in [
        ("rust", Language::Rust),
        ("typescript", Language::TypeScript),
        ("tsx", Language::Tsx),
    ] {
        let args = arguments(json!({"operation":"query","language":name,"pattern":"$A"}));
        assert_eq!(args.language, Some(expected));
    }
}

#[test]
fn ast_fixed_argv_contains_only_builtin_language_explicit_empty_config_and_literal_pattern() {
    let config = IsolatedConfig::new().unwrap();
    let pattern = "--rewrite=x; $(touch escaped) ' \n $A";
    let argv = query_arguments(&config, Language::Tsx, pattern).unwrap();
    assert_eq!(argv.len(), 6);
    assert_eq!(&argv[..3], ["run", "--stdin", "--json=compact"]);
    assert!(Path::new(argv[3].strip_prefix("--config=").unwrap()).is_absolute());
    assert_eq!(
        std::fs::read(argv[3].strip_prefix("--config=").unwrap()).unwrap(),
        b"{}\n"
    );
    assert_eq!(argv[4], "--lang=tsx");
    assert_eq!(argv[5], format!("--pattern={pattern}"));
    config.verify().unwrap();
    std::fs::write(
        argv[3].strip_prefix("--config=").unwrap(),
        "customLanguages: {}\n",
    )
    .unwrap();
    assert!(config.verify().is_err());
}

#[cfg(unix)]
#[test]
fn ast_resolver_ignores_sg_relative_workspace_nonexecutable_and_script_candidates() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let root = Scratch::new();
    let tools = Scratch::new();
    tools.program("sg", "exit 0");
    root.program("ast-grep", "exit 0");
    let path = std::env::join_paths([Path::new("."), root.0.as_path(), tools.0.as_path()]).unwrap();
    assert!(launch::executable_from_path(&root.0, &path).is_err());
    tools.program("ast-grep", "exit 0");
    assert!(
        launch::executable_from_path(&root.0, &path).is_err(),
        "script wrapper was accepted"
    );
    let binary = tools.write("ast-grep", b"\x7fELFfixture");
    std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o600)).unwrap();
    assert!(launch::executable_from_path(&root.0, &path).is_err());
    std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o700)).unwrap();
    assert_eq!(
        launch::executable_from_path(&root.0, &path).unwrap(),
        binary
    );
    std::fs::remove_file(&binary).unwrap();
    symlink(root.0.join("ast-grep"), &binary).unwrap();
    assert!(launch::executable_from_path(&root.0, &path).is_err());
}

#[test]
fn ast_snapshot_filters_languages_rejects_escapes_and_enforces_file_and_byte_budgets() {
    let root = Scratch::new();
    root.write("main.rs", "fn main() {}\n");
    root.write("main.ts", "const x = 1;\n");
    root.write("main.tsx", "const x = <div/>;\n");
    root.write(".secret.rs", "hidden");
    std::fs::create_dir(root.0.join("target")).unwrap();
    std::fs::write(root.0.join("target/generated.rs"), "ignored").unwrap();
    for (language, path) in [
        (Language::Rust, "main.rs"),
        (Language::TypeScript, "main.ts"),
        (Language::Tsx, "main.tsx"),
    ] {
        let sources = collect(&root.0, ".", language).unwrap();
        assert_eq!(sources.len(), 1);
        assert_eq!(sources[0].relative, path);
    }
    for path in [
        "../main.rs",
        "/etc/passwd",
        ".secret.rs",
        "target/generated.rs",
        "a\\b.rs",
        "",
    ] {
        assert!(
            collect(&root.0, path, Language::Rust).is_err(),
            "accepted {path}"
        );
    }
    assert!(collect(&root.0, "main.ts", Language::Rust).is_err());
    root.write("big.rs", vec![b'x'; 512 * 1024 + 1]);
    assert!(collect(&root.0, "big.rs", Language::Rust).is_err());
    std::fs::remove_file(root.0.join("big.rs")).unwrap();
    root.write("invalid.rs", [0xff]);
    assert!(collect(&root.0, "invalid.rs", Language::Rust).is_err());
    std::fs::remove_file(root.0.join("invalid.rs")).unwrap();
    for index in 0..128 {
        root.write(&format!("source{index}.rs"), "fn f() {}\n");
    }
    assert!(collect(&root.0, ".", Language::Rust).is_err());
}

#[cfg(unix)]
#[test]
fn ast_snapshot_never_reads_symlinks_or_fifos() {
    use std::os::unix::fs::symlink;
    let root = Scratch::new();
    let outside = Scratch::new();
    outside.write("outside.rs", "fn outside() {}\n");
    symlink(outside.0.join("outside.rs"), root.0.join("link.rs")).unwrap();
    symlink(&outside.0, root.0.join("linked")).unwrap();
    let fifo = std::ffi::CString::new(root.0.join("pipe.rs").to_str().unwrap()).unwrap();
    // SAFETY: CString is live and this is a unique private fixture path.
    assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);
    assert!(collect(&root.0, ".", Language::Rust).unwrap().is_empty());
    for path in ["link.rs", "linked/outside.rs", "pipe.rs"] {
        assert!(collect(&root.0, path, Language::Rust).is_err());
    }
}

#[test]
fn ast_snapshot_rejects_modified_or_replaced_files() {
    let root = Scratch::new();
    let path = root.write("main.rs", "foo(1)\n");
    let snapshot = Snapshot::read(&root.0, &path).unwrap();
    snapshot.verify(&root.0).unwrap();
    std::fs::write(&path, "foo(2)\n").unwrap();
    assert!(snapshot.verify(&root.0).is_err());
    std::fs::remove_file(&path).unwrap();
    std::fs::write(&path, "foo(1)\n").unwrap();
    #[cfg(unix)]
    assert!(
        snapshot.verify(&root.0).is_err(),
        "same bytes in a replaced inode are not the original snapshot"
    );
}

#[cfg(unix)]
fn captured(code: i32, stdout: impl AsRef<[u8]>) -> std::process::Output {
    use std::os::unix::process::ExitStatusExt;
    std::process::Output {
        status: std::process::ExitStatus::from_raw(code << 8),
        stdout: stdout.as_ref().to_vec(),
        stderr: Vec::new(),
    }
}

#[cfg(unix)]
fn match_json(start: usize, end: usize, text: &str) -> Value {
    json!({"file":"STDIN","text":text,"range":{"byteOffset":{"start":start,"end":end},"start":{"line":999,"column":999}}})
}

#[cfg(unix)]
#[test]
fn ast_output_binds_host_path_hash_and_utf8_bytes_and_rejects_forgery() {
    let root = Scratch::new();
    let path = root.write("main.rs", "// 中\nfoo(1)\n");
    let snapshot = Snapshot::read(&root.0, &path).unwrap();
    let start = snapshot.text.find("foo").unwrap();
    let good = match_json(start, start + 6, "foo(1)");
    let result =
        output::normalize(&captured(0, json!([good.clone()]).to_string()), &snapshot).unwrap();
    assert_eq!(result[0]["path"], "main.rs");
    assert_eq!(result[0]["hash"], snapshot.hash);
    assert_eq!(
        result[0]["byte_range"],
        json!({"start":start,"end":start + 6})
    );
    assert!(result[0].get("file").is_none());
    let mut external = good.clone();
    external["file"] = json!("/etc/passwd");
    let mut rewrite = good.clone();
    rewrite["replacement"] = Value::Null;
    let mut huge_offset = good.clone();
    huge_offset["range"]["byteOffset"]["end"] = json!(u64::MAX);
    for invalid in [
        external,
        rewrite,
        huge_offset,
        match_json(4, 5, "中"),
        match_json(start, start + 6, "forged"),
        match_json(start, start, ""),
    ] {
        assert!(output::normalize(&captured(0, json!([invalid]).to_string()), &snapshot).is_err());
    }
}

#[cfg(unix)]
#[test]
fn ast_output_rejects_partial_json_bad_utf8_diagnostics_and_inconsistent_exit() {
    let root = Scratch::new();
    let snapshot = Snapshot::read(&root.0, &root.write("main.rs", "foo(1)\n")).unwrap();
    for bytes in [
        b"[{".as_slice(),
        b"[1 earlier bytes omitted]\n[]".as_slice(),
        b"[]trailer".as_slice(),
        b"\xff[]".as_slice(),
        b"{}".as_slice(),
        b"".as_slice(),
    ] {
        assert!(output::normalize(&captured(0, bytes), &snapshot).is_err());
    }
    assert!(
        output::normalize(&captured(1, "[]"), &snapshot)
            .unwrap()
            .is_empty()
    );
    assert!(output::normalize(&captured(2, "[]"), &snapshot).is_err());
    assert!(
        output::normalize(
            &captured(1, json!([match_json(0, 6, "foo(1)")]).to_string()),
            &snapshot
        )
        .is_err()
    );
    let mut diagnostics = captured(1, "[]");
    diagnostics.stderr = b"configuration failed".to_vec();
    assert!(output::normalize(&diagnostics, &snapshot).is_err());
}

#[cfg(unix)]
fn quoted(path: &Path) -> String {
    format!("'{}'", path.to_str().unwrap().replace('\'', "'\\''"))
}

#[cfg(unix)]
#[tokio::test]
async fn ast_query_process_receives_fixed_argv_and_snapshot_then_detects_concurrent_changes() {
    let root = Scratch::new();
    let tools = Scratch::new();
    root.write("main.rs", "foo(1)\n");
    // A malicious workspace config is never passed to the child. It is not even
    // read by the host enumerator (only ordinary .rs source snapshots are read).
    root.write(
        "sgconfig.yml",
        "customLanguages: { evil: { libraryPath: /tmp/evil } }\n",
    );
    let response = json!([match_json(0, 6, "foo(1)")]).to_string();
    let checks = format!(
        "test \"$#\" = 6\ntest \"$1\" = run\ntest \"$2\" = --stdin\ntest \"$3\" = --json=compact\ntest \"$5\" = --lang=rust\ntest \"$6\" = '--pattern=foo($A)'\ntest \"$(cat \"${{4#--config=}}\")\" = '{{}}'\ntest \"$(cat)\" = 'foo(1)'\nprintf '%s' '{response}'"
    );
    let program = tools.program("ast-grep-fixture", &checks);
    let registry = ToolRegistry::new(&root.0, ApprovalMode::FullAccess).unwrap();
    let result: Value = serde_json::from_str(
        &registry
            .ast_grep_query(query_args(), &program)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(result["results"][0]["path"], "main.rs");
    assert_eq!(result["snapshot_verified"], true);
    let program = tools.program(
        "mutating-fixture",
        &format!(
            "cat >/dev/null\nprintf '\\n' >> {}\nprintf '%s' '{response}'",
            quoted(&root.0.join("main.rs"))
        ),
    );
    let error = registry
        .ast_grep_query(query_args(), &program)
        .await
        .unwrap_err();
    assert!(error.to_string().contains("snapshot_changed"), "{error}");
}

#[cfg(unix)]
#[tokio::test]
async fn ast_query_uses_gate_command_and_rejects_failed_process_output() {
    let root = Scratch::new();
    let tools = Scratch::new();
    root.write("main.rs", "foo(1)\n");
    let marker = tools.0.join("started");
    let program = tools.program(
        "blocked-fixture",
        &format!("printf started > {}", quoted(&marker)),
    );
    let registry = ToolRegistry::new(&root.0, ApprovalMode::FullAccess)
        .unwrap()
        .with_command_allowlist(Some(std::collections::HashSet::new()));
    assert!(matches!(
        registry
            .ast_grep_query(query_args(), &program)
            .await
            .unwrap_err(),
        ToolError::ApprovalDenied(_)
    ));
    assert!(!marker.exists());
    let registry = ToolRegistry::new(&root.0, ApprovalMode::FullAccess).unwrap();
    let program = tools.program("failed-fixture", "cat >/dev/null; printf '[]'; exit 2");
    assert!(
        registry
            .ast_grep_query(query_args(), &program)
            .await
            .is_err()
    );
    let program = tools.program("empty-fixture", "cat >/dev/null; printf '[]'; exit 1");
    let empty: Value = serde_json::from_str(
        &registry
            .ast_grep_query(query_args(), &program)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(empty["state"], "no_matches");
}

#[cfg(unix)]
struct WaitingApproval {
    entered: tokio::sync::Notify,
    resume: tokio::sync::Notify,
}

#[cfg(unix)]
#[async_trait::async_trait]
impl crate::tools::Approver for WaitingApproval {
    async fn approve(
        &self,
        _description: &str,
        _always_allow_available: bool,
    ) -> crate::tools::ApprovalDecision {
        self.entered.notify_one();
        self.resume.notified().await;
        crate::tools::ApprovalDecision::AllowOnce
    }
}

#[cfg(unix)]
#[tokio::test]
async fn ast_query_rechecks_read_only_after_waiting_for_command_approval() {
    let root = Scratch::new();
    let tools = Scratch::new();
    root.write("main.rs", "foo(1)\n");
    let marker = tools.0.join("started");
    let program = tools.program(
        "approved-fixture",
        &format!(
            "printf started > {}; cat >/dev/null; printf '[]'; exit 1",
            quoted(&marker)
        ),
    );
    let approver = std::sync::Arc::new(WaitingApproval {
        entered: tokio::sync::Notify::new(),
        resume: tokio::sync::Notify::new(),
    });
    let registry = ToolRegistry::new(&root.0, ApprovalMode::Strict)
        .unwrap()
        .with_approver(approver.clone());
    let mut running = Box::pin(registry.ast_grep_query(query_args(), &program));
    tokio::select! {
        result = &mut running => panic!("query completed before approval: {result:?}"),
        ready = tokio::time::timeout(Duration::from_secs(5), approver.entered.notified()) => ready.unwrap(),
    }
    registry.approval_mode_handle().set(ApprovalMode::ReadOnly);
    approver.resume.notify_one();
    assert!(matches!(running.await, Err(ToolError::ReadOnlyPolicy(_))));
    assert!(
        !marker.exists(),
        "approval granted under an obsolete mode started a process"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn ast_query_discards_in_flight_results_and_never_starts_next_file_after_policy_tightens() {
    for mode in [ApprovalMode::ReadOnly, ApprovalMode::Strict] {
        let root = Scratch::new();
        let tools = Scratch::new();
        root.write("a.rs", "foo(1)\n");
        root.write("b.rs", "foo(1)\n");
        let started = tools.0.join("started");
        let resume = tools.0.join("resume");
        let response = json!([match_json(0, 6, "foo(1)")]).to_string();
        let program = tools.program("waiting-fixture", &format!(
            "cat >/dev/null\nprintf x >> {}\nwhile test ! -e {}; do sleep 0.01; done\nprintf '%s' '{response}'",
            quoted(&started), quoted(&resume),
        ));
        let registry = ToolRegistry::new(&root.0, ApprovalMode::FullAccess).unwrap();
        let args = arguments(json!({"operation":"query","language":"rust","pattern":"foo($A)"}));
        let mut running = Box::pin(registry.ast_grep_query(args, &program));
        tokio::select! {
            result = &mut running => panic!("query completed before fixture barrier: {result:?}"),
            ready = tokio::time::timeout(Duration::from_secs(5), async {
                while !started.exists() { tokio::time::sleep(Duration::from_millis(10)).await; }
            }) => ready.unwrap(),
        }
        registry.approval_mode_handle().set(mode);
        std::fs::write(&resume, "resume").unwrap();
        let error = running.await.unwrap_err();
        if mode == ApprovalMode::ReadOnly {
            assert!(matches!(error, ToolError::ReadOnlyPolicy(_)), "{error}");
        } else {
            assert!(
                error.to_string().contains("execution policy changed"),
                "{error}"
            );
        }
        assert_eq!(
            std::fs::read_to_string(started).unwrap(),
            "x",
            "second file process was started"
        );
        assert_eq!(
            std::fs::read_to_string(root.0.join("a.rs")).unwrap(),
            "foo(1)\n"
        );
        assert_eq!(
            std::fs::read_to_string(root.0.join("b.rs")).unwrap(),
            "foo(1)\n"
        );
    }
}

#[cfg(unix)]
#[tokio::test]
async fn ast_query_detects_added_files_and_reports_result_limit_separately_from_raw_truncation() {
    let root = Scratch::new();
    let tools = Scratch::new();
    root.write("a.rs", "foo(1)\n");
    root.write("b.rs", "foo(1)\n");
    let response = json!([match_json(0, 6, "foo(1)")]).to_string();
    let program = tools.program(
        "bounded-fixture",
        &format!("cat >/dev/null; printf '%s' '{response}'"),
    );
    let registry = ToolRegistry::new(&root.0, ApprovalMode::FullAccess).unwrap();
    let args =
        arguments(json!({"operation":"query","language":"rust","pattern":"foo($A)","limit":1}));
    let result: Value =
        serde_json::from_str(&registry.ast_grep_query(args, &program).await.unwrap()).unwrap();
    assert_eq!(result["total_matches"], 2);
    assert_eq!(result["results"].as_array().unwrap().len(), 1);
    assert_eq!(result["truncated"], true);
    assert_eq!(result["snapshot_verified"], true);
    let program = tools.program(
        "adding-fixture",
        &format!(
            "cat >/dev/null; printf 'foo(1)' > {}; printf '[]'; exit 1",
            quoted(&root.0.join("new.rs"))
        ),
    );
    let args = arguments(json!({"operation":"query","language":"rust","pattern":"foo($A)"}));
    assert!(
        registry
            .ast_grep_query(args, &program)
            .await
            .unwrap_err()
            .to_string()
            .contains("snapshot_changed")
    );
}

#[cfg(unix)]
#[tokio::test]
async fn ast_query_cancellation_drops_private_config_and_kills_process_tree() {
    let root = Scratch::new();
    let tools = Scratch::new();
    root.write("main.rs", "foo(1)\n");
    let started = tools.0.join("started");
    let escaped = tools.0.join("escaped");
    let config_record = tools.0.join("config-path");
    let program = tools.program("waiting-fixture", &format!(
        "cat >/dev/null\nprintf '%s' \"${{4#--config=}}\" > {}\n(sleep 1; printf escaped > {}) &\nprintf ready > {}\nwait",
        quoted(&config_record), quoted(&escaped), quoted(&started)
    ));
    let registry = ToolRegistry::new(&root.0, ApprovalMode::FullAccess).unwrap();
    let mut running = Box::pin(registry.ast_grep_query(query_args(), &program));
    tokio::select! {
        result = &mut running => panic!("unexpected completion: {result:?}"),
        _ = async {
            tokio::time::timeout(Duration::from_secs(5), async {
                while !started.exists() { tokio::time::sleep(Duration::from_millis(10)).await; }
            }).await.unwrap();
        } => {}
    }
    let config = PathBuf::from(std::fs::read_to_string(&config_record).unwrap());
    assert!(config.exists());
    drop(running);
    assert!(!config.exists());
    assert!(!config.parent().unwrap().exists());
    tokio::time::sleep(Duration::from_millis(1200)).await;
    assert!(!escaped.exists(), "query cancellation leaked a descendant");
}

#[test]
fn ast_snapshot_rejects_total_source_budget_overflow() {
    let root = Scratch::new();
    let source = vec![b'x'; 512 * 1024];
    for index in 0..9 {
        root.write(&format!("large{index}.rs"), &source);
    }
    assert!(
        collect(&root.0, ".", Language::Rust)
            .unwrap_err()
            .to_string()
            .contains("byte budget")
    );
}

#[tokio::test]
async fn ast_status_is_offline_and_uses_no_process_or_workspace_config() {
    let root = Scratch::new();
    root.write("sgconfig.yml", "not valid configuration: [");
    let registry = ToolRegistry::new(&root.0, ApprovalMode::ReadOnly)
        .unwrap()
        .with_command_allowlist(Some(std::collections::HashSet::new()));
    let status: Value = serde_json::from_str(
        &registry
            .ast_grep(json!({"operation":"status"}))
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(status["process_started"], false);
    assert_eq!(status["configuration_isolation"]["config_discovery"], false);
    assert!(matches!(
        status["state"].as_str(),
        Some("dependency_missing" | "available_unprobed")
    ));
    assert_eq!(std::fs::read_dir(&root.0).unwrap().count(), 1);
}
