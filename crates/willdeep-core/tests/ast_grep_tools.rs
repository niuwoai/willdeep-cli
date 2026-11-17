//! Public Runtime wiring checks and opt-in smoke queries against a trusted,
//! preinstalled ast-grep. These tests never install or download a dependency.
use std::path::PathBuf;

use serde_json::{Value, json};
use willdeep_core::{ApprovalMode, ToolCall, ToolRegistry};

struct Workspace(PathBuf);

impl Workspace {
    fn new() -> Self {
        let root =
            std::env::temp_dir().join(format!("willdeep-ast-public-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        Self(root.canonicalize().unwrap())
    }
}

impl Drop for Workspace {
    fn drop(&mut self) {
        if let Err(error) = std::fs::remove_dir_all(&self.0) {
            eprintln!("remove private ast-grep smoke workspace: {error}");
        }
    }
}

async fn execute(registry: &ToolRegistry, arguments: Value) -> Result<Value, String> {
    let text = registry
        .execute(&ToolCall {
            id: uuid::Uuid::new_v4().to_string(),
            name: "ast_grep".into(),
            arguments: arguments.to_string(),
        })
        .await
        .map_err(|e| e.to_string())?;
    serde_json::from_str(&text).map_err(|e| e.to_string())
}

#[tokio::test]
async fn ast_grep_definition_and_offline_status_are_exposed_by_shared_registry() {
    let workspace = Workspace::new();
    std::fs::write(workspace.0.join("sgconfig.yml"), "malformed: [").unwrap();
    let registry = ToolRegistry::new(&workspace.0, ApprovalMode::FullAccess).unwrap();
    let definitions = registry.definitions();
    let definition = definitions
        .iter()
        .find(|d| d.name == "ast_grep")
        .expect("shared tool definition wired");
    assert_eq!(definition.parameters["additionalProperties"], false);
    let status = execute(&registry, json!({"operation":"status"}))
        .await
        .unwrap();
    assert_eq!(status["process_started"], false);
    assert!(status["dependency"]["available"].is_boolean());
    assert_eq!(status["configuration_isolation"]["config_discovery"], false);
    assert_eq!(std::fs::read_dir(&workspace.0).unwrap().count(), 1);
}

#[tokio::test]
async fn ast_grep_public_dispatch_rejects_user_execution_and_configuration_fields() {
    let workspace = Workspace::new();
    let registry = ToolRegistry::new(&workspace.0, ApprovalMode::FullAccess).unwrap();
    for key in ["config", "command", "args", "rewrite", "parser", "install"] {
        let mut value = json!({"operation":"query","language":"rust","pattern":"foo($A)"});
        value[key] = json!("arbitrary");
        let error = execute(&registry, value).await.unwrap_err();
        assert!(error.contains("invalid arguments"), "{key}: {error}");
    }
}

#[tokio::test]
#[ignore = "requires explicitly selected execution and a trusted native ast-grep on PATH"]
async fn real_ast_grep_rust_typescript_tsx_use_stdin_empty_config_and_host_snapshots() {
    let workspace = Workspace::new();
    // If discovered, this malformed project configuration would make the smoke
    // fail. The query must instead pass its private empty --config explicitly.
    std::fs::write(
        workspace.0.join("sgconfig.yml"),
        "customLanguages: [invalid",
    )
    .unwrap();
    let cases = [
        (
            "rust",
            "main.rs",
            "fn main() { println!(\"hello\"); }\n",
            "println!($A)",
        ),
        (
            "typescript",
            "main.ts",
            "console.log('hello');\n",
            "console.log($A)",
        ),
        (
            "tsx",
            "main.tsx",
            "const node = <div>hello</div>;\n",
            "<div>$A</div>",
        ),
    ];
    for (_, path, source, _) in &cases {
        std::fs::write(workspace.0.join(path), source).unwrap();
    }
    let registry = ToolRegistry::new(&workspace.0, ApprovalMode::FullAccess).unwrap();
    let status = execute(&registry, json!({"operation":"status"}))
        .await
        .unwrap();
    assert_eq!(status["dependency"]["available"], true, "{status}");
    for (language, path, source, pattern) in cases {
        let result = execute(
            &registry,
            json!({
                "operation":"query","path":path,"language":language,"pattern":pattern,
            }),
        )
        .await
        .unwrap();
        assert_eq!(result["snapshot_verified"], true, "{result}");
        let matches = result["results"].as_array().unwrap();
        assert!(!matches.is_empty(), "{result}");
        for item in matches {
            assert_eq!(item["path"], path);
            assert!(item["hash"].as_str().unwrap().starts_with("sha256:"));
            let start = item["byte_range"]["start"].as_u64().unwrap() as usize;
            let end = item["byte_range"]["end"].as_u64().unwrap() as usize;
            assert_eq!(source.get(start..end), item["text"].as_str());
        }
        let empty = execute(&registry, json!({
            "operation":"query","path":path,"language":language,"pattern":"unrelated_identifier_7654321",
        })).await.unwrap();
        assert_eq!(empty["state"], "no_matches", "{empty}");
        assert!(empty["results"].as_array().unwrap().is_empty());
    }
}
