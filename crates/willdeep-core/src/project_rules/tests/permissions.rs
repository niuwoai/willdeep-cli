use super::*;
use std::os::unix::fs::PermissionsExt;

struct DeniedPath {
    path: PathBuf,
    permissions: std::fs::Permissions,
}

impl DeniedPath {
    fn new(path: &Path) -> Self {
        let permissions = std::fs::metadata(path).unwrap().permissions();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o000)).unwrap();
        Self {
            path: path.to_owned(),
            permissions,
        }
    }

    fn is_enforced(&self, probe: std::io::Result<()>) -> bool {
        match probe {
            Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => true,
            Ok(()) => {
                eprintln!(
                    "SKIP permission fixture {}: this process can bypass mode bits; error classification tests still run",
                    self.path.display()
                );
                false
            }
            Err(error) => panic!(
                "unexpected permission probe for {}: {error}",
                self.path.display()
            ),
        }
    }
}

impl Drop for DeniedPath {
    fn drop(&mut self) {
        if let Err(error) = std::fs::set_permissions(&self.path, self.permissions.clone()) {
            eprintln!(
                "restore fixture permissions for {}: {error}",
                self.path.display()
            );
        }
    }
}

#[test]
fn opaque_tools_skip_unreadable_unrelated_directories() {
    let root = workspace();
    let private = root.join(".locked");
    std::fs::create_dir(&private).unwrap();
    std::fs::write(private.join("AGENTS.md"), "Private scope").unwrap();
    std::fs::write(root.join("other/AGENTS.md"), "Readable scope").unwrap();
    let denied = DeniedPath::new(&private);
    if !denied.is_enforced(std::fs::read_dir(&private).map(drop)) {
        return;
    }

    for name in OPAQUE_TOOLS {
        let mut rules = ProjectRules::new(&root).unwrap();
        let tool = ToolCall {
            name: name.into(),
            ..call(".")
        };
        assert!(rules.before_call(&tool).unwrap(), "{name}");
        assert!(rules.render().contains("Source: other/AGENTS.md"));
        assert!(!rules.render().contains("Private scope"));
        assert!(!rules.before_call(&tool).unwrap(), "{name}");
    }

    let mut rules = ProjectRules::new(&root).unwrap();
    let tool = ToolCall {
        name: "run_command".into(),
        ..call(".")
    };
    assert!(rules.before_call(&tool).unwrap());
    drop(denied);
    assert!(rules.before_call(&tool).unwrap());
    assert!(rules.render().contains("Private scope"));
    assert!(!rules.before_call(&tool).unwrap());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn an_unlistable_but_searchable_directory_still_blocks_opaque_tools() {
    let root = workspace();
    let scope = root.join("other");
    std::fs::write(scope.join("AGENTS.md"), "Required searchable rule").unwrap();
    let denied = DeniedPath::new(&scope);
    std::fs::set_permissions(&scope, std::fs::Permissions::from_mode(0o111)).unwrap();
    if !denied.is_enforced(std::fs::read_dir(&scope).map(drop)) {
        return;
    }
    assert_eq!(
        std::fs::read_to_string(scope.join("AGENTS.md")).unwrap(),
        "Required searchable rule"
    );
    for name in OPAQUE_TOOLS {
        let mut rules = ProjectRules::new(&root).unwrap();
        assert!(
            rules
                .before_call(&ToolCall {
                    name: name.into(),
                    ..call(".")
                })
                .is_err(),
            "{name}"
        );
    }
    drop(denied);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn unreadable_rule_files_still_block_opaque_tools() {
    let root = workspace();
    for name in RULE_NAMES {
        let path = root.join("other").join(name);
        std::fs::write(&path, "Required rule").unwrap();
        for previously_loaded in [false, true] {
            let mut rules = ProjectRules::new(&root).unwrap();
            let tool = ToolCall {
                name: "run_command".into(),
                ..call(".")
            };
            if previously_loaded {
                assert!(rules.before_call(&tool).unwrap());
            }
            let denied = DeniedPath::new(&path);
            if !denied.is_enforced(std::fs::File::open(&path).map(drop)) {
                return;
            }
            assert_eq!(
                rules.before_call(&tool).unwrap_err().kind(),
                std::io::ErrorKind::PermissionDenied,
                "{name}, previously_loaded={previously_loaded}"
            );
        }
        std::fs::remove_file(path).unwrap();
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn a_previously_discovered_scope_cannot_be_hidden_by_permissions() {
    let root = workspace();
    let scope = root.join("other");
    std::fs::write(scope.join("AGENTS.md"), "Required rule").unwrap();
    let mut rules = ProjectRules::new(&root).unwrap();
    let tool = ToolCall {
        name: "run_command".into(),
        ..call(".")
    };
    assert!(rules.before_call(&tool).unwrap());
    let denied = DeniedPath::new(&scope);
    if !denied.is_enforced(std::fs::read_dir(&scope).map(drop)) {
        return;
    }
    assert_eq!(
        rules.before_call(&tool).unwrap_err().kind(),
        std::io::ErrorKind::PermissionDenied
    );
    assert!(rules.render().contains("Required rule"));
    drop(denied);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn an_unreadable_workspace_root_still_blocks_discovery() {
    let root = workspace();
    let mut rules = ProjectRules::new(&root).unwrap();
    let denied = DeniedPath::new(&root);
    if !denied.is_enforced(std::fs::read_dir(&root).map(drop)) {
        return;
    }
    assert!(
        rules
            .before_call(&ToolCall {
                name: "run_command".into(),
                ..call(".")
            })
            .is_err()
    );
    drop(denied);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn directory_symlink_errors_cannot_be_skipped() {
    let root = workspace();
    let rules = ProjectRules::new(&root).unwrap();
    let link = rules.workspace.join("link");
    std::os::unix::fs::symlink(rules.workspace.join("other"), &link).unwrap();
    let error = discovery_error(&link, 1, std::io::ErrorKind::PermissionDenied);
    assert!(!rules.can_skip_discovery_error(&error));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn opaque_discovery_rejects_nested_rule_symlinks_outside_the_workspace() {
    let root = workspace();
    let outside = workspace();
    let mut rules = ProjectRules::new(&root).unwrap();
    for name in RULE_NAMES {
        let path = root.join("other").join(name);
        std::fs::write(outside.join("rules.md"), "Outside rule").unwrap();
        std::os::unix::fs::symlink(outside.join("rules.md"), &path).unwrap();
        assert!(
            rules
                .before_call(&ToolCall {
                    name: "run_command".into(),
                    ..call(".")
                })
                .is_err()
        );
        std::fs::remove_file(path).unwrap();
    }
    std::fs::remove_dir_all(root).unwrap();
    std::fs::remove_dir_all(outside).unwrap();
}
