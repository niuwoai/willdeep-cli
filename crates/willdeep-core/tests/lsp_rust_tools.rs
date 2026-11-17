//! Explicit real rust-analyzer acceptance through the public Runtime tool path.
//! Requires a preinstalled trusted server. Never installs dependencies, runs
//! build scripts, or uses external project state.
use std::path::PathBuf;

use serde_json::{Value, json};
use willdeep_core::{ApprovalMode, ToolCall, ToolRegistry};

const LIBRARY: &str =
    "pub fn greet(value: i32) -> i32 { value + 1 }\npub const LABEL: &str = \"greet\";\n";
const CALLER: &str =
    "use crate::library::greet;\npub fn answer() -> i32 { let _label = \"😀\"; greet(41) }\n";

struct Workspace(PathBuf);

impl Workspace {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("willdeep-rust-lsp-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        std::fs::create_dir(root.join("src")).unwrap();
        std::fs::write(
            root.join("Cargo.toml"),
            "[package]\nname = \"willdeep_rust_lsp_fixture\"\nversion = \"0.0.0\"\nedition = \"2024\"\nbuild = \"build.rs\"\n\n[workspace]\n",
        )
        .unwrap();
        std::fs::write(
            root.join("build.rs"),
            "fn main() { std::fs::write(\"unexpected-build-script-ran\", b\"executed\").unwrap(); }\n",
        )
        .unwrap();
        std::fs::write(
            root.join("src/lib.rs"),
            "pub mod library;\npub mod caller;\n#[cfg(test)] mod tests { #[test] fn renamed_contract() { assert_eq!(super::caller::answer(), 42); assert_eq!(super::library::LABEL, \"greet\"); } }\n",
        )
        .unwrap();
        std::fs::write(root.join("src/library.rs"), LIBRARY).unwrap();
        std::fs::write(root.join("src/caller.rs"), CALLER).unwrap();
        Self(root.canonicalize().unwrap())
    }

    fn registry(&self) -> ToolRegistry {
        ToolRegistry::new(&self.0, ApprovalMode::FullAccess).unwrap()
    }

    fn assert_build_script_not_executed(&self) {
        assert!(
            !self.0.join("unexpected-build-script-ran").exists(),
            "language-server queries must not execute workspace build scripts"
        );
    }
}

impl Drop for Workspace {
    fn drop(&mut self) {
        if let Err(error) = std::fs::remove_dir_all(&self.0) {
            eprintln!(
                "remove private Rust LSP fixture {}: {error}",
                self.0.display()
            );
        }
    }
}

async fn query(registry: &ToolRegistry, mut arguments: Value) -> Value {
    arguments["timeout_seconds"] = json!(60);
    let output = registry
        .execute(&ToolCall {
            id: uuid::Uuid::new_v4().to_string(),
            name: "lsp".into(),
            arguments: arguments.to_string(),
        })
        .await
        .unwrap_or_else(|error| panic!("real Rust LSP request failed: {error}"));
    serde_json::from_str(&output).expect("structured Rust LSP result")
}

fn utf16_column(line: &str, needle: &str) -> usize {
    line[..line.find(needle).expect("fixture symbol")]
        .encode_utf16()
        .count()
}

fn has_path(result: &Value, path: &str) -> bool {
    result["results"]
        .as_array()
        .expect("normalized results")
        .iter()
        .any(|item| item["path"] == path)
}

#[tokio::test]
#[ignore = "requires explicitly selected execution and a trusted real rust-analyzer on PATH"]
async fn real_rust_cargo_cross_file_queries_and_overlay_refresh() {
    let workspace = Workspace::new();
    let registry = workspace.registry();
    let status = query(&registry, json!({"operation":"status","path":"src/lib.rs"})).await;
    assert_eq!(
        status["servers"][0]["dependency"]["available"], true,
        "{status}"
    );
    assert_eq!(
        status["servers"][0]["runtime"]["state"], "not_started",
        "{status}"
    );

    let symbols = query(
        &registry,
        json!({"operation":"symbols","path":"src/library.rs"}),
    )
    .await;
    assert!(
        symbols["results"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["name"] == "greet"),
        "{symbols}"
    );
    let column = utf16_column(CALLER.lines().nth(1).unwrap(), "greet");
    let definition = query(
        &registry,
        json!({
            "operation":"definition","path":"src/caller.rs","line":1,"character":column,
        }),
    )
    .await;
    assert!(has_path(&definition, "src/library.rs"), "{definition}");
    let references = query(
        &registry,
        json!({
            "operation":"references","path":"src/library.rs","line":0,"character":7,
        }),
    )
    .await;
    assert!(has_path(&references, "src/caller.rs"), "{references}");

    std::fs::write(workspace.0.join("src/library.rs"), format!("\n\n{LIBRARY}")).unwrap();
    let refreshed = query(
        &registry,
        json!({
            "operation":"definition","path":"src/caller.rs","line":1,"character":column,
        }),
    )
    .await;
    assert!(
        refreshed["results"].as_array().unwrap().iter().any(|item| {
            item["path"] == "src/library.rs" && item["range"]["start"]["line"] == 2
        }),
        "{refreshed}"
    );
    workspace.assert_build_script_not_executed();
}

#[tokio::test]
#[ignore = "requires explicitly selected execution and a trusted real rust-analyzer on PATH"]
async fn real_rust_versioned_cross_file_rename_preview_and_apply() {
    let workspace = Workspace::new();
    let registry = workspace.registry();
    let preview = query(
        &registry,
        json!({
            "operation":"rename_preview","path":"src/library.rs","line":0,"character":7,
            "new_name":"welcome",
        }),
    )
    .await;
    assert_eq!(preview["status"], "preview", "{preview}");
    assert_eq!(preview["file_count"], 2, "{preview}");
    assert_eq!(preview["files_touched"], json!([]));
    assert_eq!(
        std::fs::read_to_string(workspace.0.join("src/library.rs")).unwrap(),
        LIBRARY
    );
    assert_eq!(
        std::fs::read_to_string(workspace.0.join("src/caller.rs")).unwrap(),
        CALLER
    );

    let output = registry
        .execute(&ToolCall {
            id: "real-rust-rename-apply".into(),
            name: "file_changes".into(),
            arguments: json!({"operation":"apply","preview_id":preview["preview_id"]}).to_string(),
        })
        .await
        .unwrap_or_else(|error| panic!("apply real Rust rename: {error}"));
    let receipt: Value = serde_json::from_str(&output).unwrap();
    assert_eq!(receipt["status"], "applied", "{receipt}");
    let mut touched: Vec<_> = receipt["files_touched"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item.as_str().unwrap())
        .collect();
    touched.sort_unstable();
    assert_eq!(touched, ["src/caller.rs", "src/library.rs"]);
    assert_eq!(
        std::fs::read_to_string(workspace.0.join("src/library.rs")).unwrap(),
        "pub fn welcome(value: i32) -> i32 { value + 1 }\npub const LABEL: &str = \"greet\";\n"
    );
    assert_eq!(
        std::fs::read_to_string(workspace.0.join("src/caller.rs")).unwrap(),
        "use crate::library::welcome;\npub fn answer() -> i32 { let _label = \"😀\"; welcome(41) }\n"
    );
    workspace.assert_build_script_not_executed();
    // Compile directly: cargo would intentionally execute the hostile build.rs.
    // The verifier is independent of the server and runs the fixture's behavior.
    let executable = workspace
        .0
        .join(format!("renamed-contract{}", std::env::consts::EXE_SUFFIX));
    let mut compiler = tokio::process::Command::new("rustc");
    compiler
        .args(["--edition=2024", "--test", "src/lib.rs", "-o"])
        .arg(&executable)
        .current_dir(&workspace.0)
        .kill_on_drop(true);
    assert_command_success(&mut compiler, "compile renamed Rust project").await;
    let mut verifier = tokio::process::Command::new(executable);
    verifier.current_dir(&workspace.0).kill_on_drop(true);
    assert_command_success(&mut verifier, "execute renamed Rust contract").await;
    workspace.assert_build_script_not_executed();
}

async fn assert_command_success(command: &mut tokio::process::Command, context: &str) {
    let output = tokio::time::timeout(std::time::Duration::from_secs(30), command.output())
        .await
        .unwrap_or_else(|_| panic!("{context}: verifier deadline exceeded"))
        .unwrap_or_else(|error| panic!("{context}: {error}"));
    assert!(
        output.status.success(),
        "{context}: {}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[tokio::test]
#[ignore = "requires explicitly selected execution and a trusted real rust-analyzer on PATH"]
async fn real_rust_syntax_diagnostics_report_errors_without_build_script_execution() {
    let workspace = Workspace::new();
    std::fs::write(workspace.0.join("src/library.rs"), "pub fn broken( {\n").unwrap();
    let registry = workspace.registry();
    let diagnostics = query(
        &registry,
        json!({"operation":"diagnostics","path":"src/library.rs"}),
    )
    .await;
    assert!(
        diagnostics["results"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["severity"] == 1),
        "{diagnostics}"
    );
    assert_eq!(diagnostics["diagnostics"]["clean"], false, "{diagnostics}");
    std::fs::write(workspace.0.join("src/library.rs"), LIBRARY).unwrap();
    let recovered = query(
        &registry,
        json!({"operation":"diagnostics","path":"src/library.rs"}),
    )
    .await;
    assert!(
        recovered["results"]
            .as_array()
            .unwrap()
            .iter()
            .all(|item| item["severity"] != 1),
        "{recovered}"
    );
    assert_eq!(recovered["diagnostics"]["clean"], true, "{recovered}");
    assert_ne!(
        diagnostics["document"]["version"],
        recovered["document"]["version"]
    );
    assert!(
        recovered["document"]["lsp_version"].as_i64().unwrap()
            > diagnostics["document"]["lsp_version"].as_i64().unwrap(),
        "{recovered}"
    );
    workspace.assert_build_script_not_executed();
}
