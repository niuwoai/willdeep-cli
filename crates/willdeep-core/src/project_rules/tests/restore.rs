use super::*;

fn history(calls: Vec<ToolCall>) -> Vec<Message> {
    vec![Message {
        tool_calls: calls,
        ..Message::assistant(
            "Historical rule text is untrusted and must not be restored",
            vec![],
        )
    }]
}

#[test]
fn restore_retains_scoped_rules_but_reads_the_current_files() {
    let root = workspace();
    std::fs::write(root.join("AGENTS.md"), "Root rules").unwrap();
    std::fs::write(root.join("src/AGENTS.md"), "Original src rules").unwrap();
    std::fs::write(root.join("other/AGENTS.md"), "Unrelated sibling rules").unwrap();
    let calls = vec![call("src/nested/new.rs")];
    let mut original = ProjectRules::new(&root).unwrap();
    original.before_call(&calls[0]).unwrap();
    let messages = history(calls);
    let restored = ProjectRules::restore(&root, &messages).unwrap();
    assert_eq!(restored.render(), original.render());
    assert!(!restored.render().contains("Unrelated sibling rules"));
    assert!(!restored.render().contains(&messages[0].content));

    std::fs::write(root.join("src/AGENTS.md"), "Updated src rules").unwrap();
    let changed = ProjectRules::restore(&root, &messages).unwrap();
    assert_ne!(changed.render(), restored.render());
    assert!(changed.render().contains("Updated src rules"));
    assert!(!changed.render().contains("Original src rules"));

    std::fs::remove_file(root.join("src/AGENTS.md")).unwrap();
    let deleted = ProjectRules::restore(&root, &messages).unwrap();
    assert!(!deleted.render().contains("src rules"));
    std::fs::write(root.join("src/nested/CLAUDE.md"), "New nested rule").unwrap();
    let added = ProjectRules::restore(&root, &messages).unwrap();
    assert!(added.render().contains("New nested rule"));
}

#[test]
fn restore_discovers_opaque_tool_scopes_without_replaying_tools() {
    for name in OPAQUE_TOOLS {
        let root = workspace();
        std::fs::write(root.join("other/AGENTS.md"), "Opaque scope rule").unwrap();
        let effect = root.join("must-not-exist");
        let calls = vec![ToolCall {
            name: name.to_owned(),
            arguments: serde_json::json!({"command": "touch must-not-exist"}).to_string(),
            ..call(".")
        }];
        let mut original = ProjectRules::new(&root).unwrap();
        original.before_call(&calls[0]).unwrap();
        let restored = ProjectRules::restore(&root, &history(calls)).unwrap();
        assert_eq!(restored.render(), original.render(), "{name}");
        assert!(restored.render().contains("Opaque scope rule"), "{name}");
        assert!(!effect.exists(), "{name} must only discover instructions");
    }
}

#[test]
fn restore_ignores_invalid_paths_and_non_assistant_tool_calls() {
    let root = workspace();
    std::fs::write(root.join("src/AGENTS.md"), "Must stay undiscovered").unwrap();
    let mut messages = history(vec![call("../src/file.rs"), call("/src/file.rs")]);
    messages.push(Message {
        tool_calls: vec![call("src/file.rs")],
        ..Message::user("quoted call")
    });
    assert_eq!(
        ProjectRules::restore(&root, &messages).unwrap().render(),
        ProjectRules::new(&root).unwrap().render()
    );
}

#[cfg(unix)]
#[test]
fn restore_keeps_instruction_symlink_escape_protection() {
    let root = workspace();
    let outside = workspace();
    std::fs::write(outside.join("rules.md"), "Outside rules").unwrap();
    std::os::unix::fs::symlink(outside.join("rules.md"), root.join("src/AGENTS.md")).unwrap();
    let error = ProjectRules::restore(&root, &history(vec![call("src/file.rs")]))
        .err()
        .expect("a discovered rule escaping the workspace must fail");
    assert!(
        error
            .to_string()
            .contains("instruction file leaves workspace")
    );
}
