//! Opt-in smoke coverage against a real, preinstalled language server.
//! This is intentionally ignored in the deterministic unit suite.
use std::path::PathBuf;

use serde_json::{Value, json};
use willdeep_core::{ApprovalMode, ToolCall, ToolRegistry};

struct Workspace(PathBuf);

impl Workspace {
    fn new() -> Self {
        let root =
            std::env::temp_dir().join(format!("willdeep-lsp-smoke-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).expect("create private smoke workspace");
        Self(root)
    }
}

impl Drop for Workspace {
    fn drop(&mut self) {
        if let Err(error) = std::fs::remove_dir_all(&self.0) {
            eprintln!(
                "remove private LSP smoke workspace {}: {error}",
                self.0.display()
            );
        }
    }
}

async fn query(registry: &ToolRegistry, arguments: Value) -> Value {
    let output = registry
        .execute(&ToolCall {
            id: uuid::Uuid::new_v4().to_string(),
            name: "lsp".into(),
            arguments: arguments.to_string(),
        })
        .await
        .expect("real TypeScript LSP query");
    serde_json::from_str(&output).expect("structured LSP result")
}

#[tokio::test]
#[ignore = "requires an explicitly selected run and a trusted external TypeScript language server"]
async fn real_typescript_definition_references_symbols_and_overlay_refresh() {
    smoke_typescript_project(true).await;
}

#[tokio::test]
#[ignore = "requires an explicitly selected run and a trusted external TypeScript language server"]
async fn real_typescript_inferred_project_cross_file_queries() {
    smoke_typescript_project(false).await;
}

#[tokio::test]
#[ignore = "requires an explicitly selected run and a trusted external TypeScript language server; never installs dependencies"]
async fn real_typescript_rename_preview_cross_file_string_key_and_utf16() {
    let workspace = Workspace::new();
    let library = "export const item = { oldKey: 1 };\nexport const unrelated = 'oldKey';\n";
    let caller = "import { item } from './library';\nconst emoji = '😀'; item['oldKey']; item.oldKey;\nexport const unrelated = 'oldKey';\n";
    std::fs::write(workspace.0.join("library.ts"), library).unwrap();
    std::fs::write(workspace.0.join("main.ts"), caller).unwrap();
    std::fs::write(workspace.0.join("tsconfig.json"),
        r#"{"compilerOptions":{"strict":true,"module":"commonjs","moduleResolution":"node"},"include":["*.ts"]}"#).unwrap();
    let registry = ToolRegistry::new(&workspace.0, ApprovalMode::FullAccess).unwrap();
    let preview = query(
        &registry,
        json!({
            "operation":"rename_preview","path":"library.ts","line":0,
            "character":library.find("oldKey").unwrap(),"new_name":"newKey","timeout_seconds":60,
        }),
    )
    .await;
    assert_eq!(preview["status"], "preview", "{preview}");
    assert_eq!(preview["file_count"], 2, "{preview}");
    assert_eq!(preview["files_touched"], json!([]));
    assert_eq!(
        std::fs::read_to_string(workspace.0.join("library.ts")).unwrap(),
        library
    );
    assert_eq!(
        std::fs::read_to_string(workspace.0.join("main.ts")).unwrap(),
        caller
    );
    let result = registry
        .execute(&ToolCall {
            id: "typescript-rename-apply".into(),
            name: "file_changes".into(),
            arguments: json!({"operation":"apply","preview_id":preview["preview_id"]}).to_string(),
        })
        .await
        .expect("apply real TypeScript rename preview");
    let receipt: Value = serde_json::from_str(&result).unwrap();
    assert_eq!(receipt["status"], "applied", "{receipt}");
    let mut touched: Vec<_> = receipt["files_touched"]
        .as_array()
        .unwrap()
        .iter()
        .map(|value| value.as_str().unwrap())
        .collect();
    touched.sort_unstable();
    assert_eq!(touched, ["library.ts", "main.ts"]);
    assert_eq!(
        std::fs::read_to_string(workspace.0.join("library.ts")).unwrap(),
        "export const item = { newKey: 1 };\nexport const unrelated = 'oldKey';\n"
    );
    assert_eq!(
        std::fs::read_to_string(workspace.0.join("main.ts")).unwrap(),
        "import { item } from './library';\nconst emoji = '😀'; item['newKey']; item.newKey;\nexport const unrelated = 'oldKey';\n"
    );
    // Use the real trusted compiler as an independent verifier of both files.
    let mut compiler = tokio::process::Command::new("tsc");
    compiler
        .args(["--noEmit", "--project", "tsconfig.json"])
        .current_dir(&workspace.0)
        .env_remove("NODE_OPTIONS")
        .env_remove("NODE_PATH")
        .kill_on_drop(true);
    let output = tokio::time::timeout(std::time::Duration::from_secs(30), compiler.output())
        .await
        .expect("TypeScript compiler deadline")
        .expect("start trusted preinstalled tsc");
    assert!(
        output.status.success(),
        "renamed TypeScript project failed type checking: {}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[tokio::test]
#[ignore = "requires explicitly selected execution and trusted external TypeScript language server and compiler; never installs dependencies"]
async fn real_typescript_diagnostics_invalidate_and_recover_with_document_versions() {
    let workspace = Workspace::new();
    let path = workspace.0.join("main.ts");
    let invalid = "export const value: number = 'wrong type';\n";
    let valid = "export const value: number = 42;\n";
    std::fs::write(&path, invalid).unwrap();
    std::fs::write(
        workspace.0.join("tsconfig.json"),
        r#"{"compilerOptions":{"strict":true,"noEmit":true},"include":["*.ts"]}"#,
    )
    .unwrap();
    let registry = ToolRegistry::new(&workspace.0, ApprovalMode::FullAccess).unwrap();
    let mut previous_version = 0;
    let mut previous_snapshot = Value::Null;
    for (text, expect_error) in [(invalid, true), (valid, false), (invalid, true)] {
        std::fs::write(&path, text).unwrap();
        let report = query(
            &registry,
            json!({"operation":"diagnostics","path":"main.ts","timeout_seconds":30}),
        )
        .await;
        let version = report["document"]["lsp_version"].as_i64().unwrap();
        assert!(version > previous_version, "{report}");
        assert_ne!(report["document"]["version"], previous_snapshot, "{report}");
        let errors: Vec<_> = report["results"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|item| item["severity"] == 1)
            .collect();
        assert_eq!(!errors.is_empty(), expect_error, "{report}");
        if expect_error {
            assert!(errors.iter().any(|item| item["code"] == 2322), "{report}");
            assert_eq!(report["diagnostics"]["clean"], false, "{report}");
        } else if report["diagnostics"]["version_verified"] == true {
            assert_eq!(report["diagnostics"]["clean"], true, "{report}");
        } else {
            assert!(report["diagnostics"]["clean"].is_null(), "{report}");
        }
        if report["diagnostics"]["transport"] == "push"
            && report["diagnostics"]["version_verified"] == true
        {
            assert_eq!(
                report["diagnostics"]["published_version"], version,
                "{report}"
            );
        }
        for item in report["results"].as_array().unwrap() {
            assert_eq!(item["version"], report["document"]["version"], "{report}");
            assert_eq!(item["lsp_version"], version, "{report}");
        }
        previous_version = version;
        previous_snapshot = report["document"]["version"].clone();
    }
}

async fn smoke_typescript_project(configured: bool) {
    let workspace = Workspace::new();
    let library = "export function greet(name: string): string { return name; }\n";
    let caller = "import { greet } from './library';\nexport const output = greet('hello');\n";
    std::fs::write(workspace.0.join("library.ts"), library).unwrap();
    std::fs::write(workspace.0.join("main.ts"), caller).unwrap();
    if configured {
        std::fs::write(
            workspace.0.join("tsconfig.json"),
            r#"{"compilerOptions":{"strict":true,"module":"commonjs","moduleResolution":"node"},"include":["*.ts"]}"#,
        )
        .unwrap();
    }
    let registry = ToolRegistry::new(&workspace.0, ApprovalMode::FullAccess).unwrap();
    let status = query(&registry, json!({"operation":"status","path":"main.ts"})).await;
    assert_eq!(
        status["servers"][0]["dependency"]["available"], true,
        "{status}"
    );
    assert_eq!(
        status["servers"][0]["runtime"]["state"], "not_started",
        "status must not spawn a server"
    );

    let definition = query(
        &registry,
        json!({
            "operation":"definition","path":"main.ts","line":1,
            "character":caller.lines().nth(1).unwrap().find("greet").unwrap(),"timeout_seconds":30,
        }),
    )
    .await;
    assert!(
        definition["results"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["path"] == "library.ts"),
        "{definition}"
    );
    let references = query(
        &registry,
        json!({
            "operation":"references","path":"library.ts","line":0,
            "character":library.find("greet").unwrap(),"timeout_seconds":30,
        }),
    )
    .await;
    assert!(
        references["results"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["path"] == "main.ts"),
        "{references}"
    );
    let symbols = query(
        &registry,
        json!({"operation":"symbols","path":"library.ts","timeout_seconds":30}),
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

    // Both files have now been opened. Refresh a previously opened overlay while
    // querying the other file, rather than trusting the server's stale buffer.
    std::fs::write(
        workspace.0.join("library.ts"),
        "\n\nexport function greet(name: string): string { return name; }\n",
    )
    .unwrap();
    let refreshed = query(
        &registry,
        json!({
            "operation":"definition","path":"main.ts","line":1,
            "character":caller.lines().nth(1).unwrap().find("greet").unwrap(),"timeout_seconds":30,
        }),
    )
    .await;
    assert!(
        refreshed["results"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| { item["path"] == "library.ts" && item["range"]["start"]["line"] == 2 }),
        "{refreshed}"
    );
    drop(registry);
}
