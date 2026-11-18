use super::*;
use willdeep_core::tools::covered_paths_changed;

struct Fixture(std::path::PathBuf);

impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "willdeep-coverage-regression-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let fixture = Self(root.canonicalize().unwrap());
        fixture.git(&["init", "--quiet"]);
        fixture.git(&["config", "user.name", "Verification Fixture"]);
        fixture.git(&["config", "user.email", "fixture@willdeep.invalid"]);
        fixture.git(&["config", "commit.gpgsign", "false"]);
        fixture.git(&["config", "core.hooksPath", ".git/no-test-hooks"]);
        fixture
    }

    fn write(&self, path: &str, content: &str) {
        let path = self.0.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
    }

    fn git(&self, args: &[&str]) {
        let output = Command::new("git")
            .args(args)
            .current_dir(&self.0)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn commit(&self) {
        self.git(&["add", "."]);
        self.git(&["commit", "--quiet", "-m", "fixture"]);
    }

    fn package(&self, base: &str, name: &str, extra: &str) {
        self.write(
            &format!("{base}/Cargo.toml"),
            &format!(
                "[package]\nname = \"{name}\"\nversion = \"0.0.0\"\nedition = \"2021\"\n{extra}"
            ),
        );
        self.write(
            &format!("{base}/src/lib.rs"),
            "pub fn value() -> u8 { 1 }\n",
        );
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        if let Err(error) = std::fs::remove_dir_all(&self.0) {
            eprintln!("Cannot remove fixture {}: {error}", self.0.display());
        }
    }
}

fn changed(
    command: &str,
    before: &willdeep_core::VerificationWorkspaceView,
    after: &willdeep_core::VerificationWorkspaceView,
) -> bool {
    assert_ne!(before.snapshot_id, after.snapshot_id);
    covered_paths_changed(
        command,
        before.dirty_fingerprints.as_ref().unwrap(),
        after.dirty_fingerprints.as_ref().unwrap(),
        before.package_dirs.as_ref(),
    )
}

#[test]
fn root_package_empty_prefix_covers_real_git_source_changes() {
    let fixture = Fixture::new();
    fixture.package(".", "root-app", "");
    assert!(package_dirs(&fixture.0).unwrap()["root-app"].contains(&String::new()));
    fixture.commit();
    let before = workspace_view(&fixture.0);
    fixture.write("src/lib.rs", "pub fn value() -> u8 { 2 }\n");
    let after = workspace_view(&fixture.0);
    assert!(changed("cargo test -p root-app", &before, &after));
}

#[test]
fn cargo_subdirectory_and_git_fingerprints_use_one_coordinate_system() {
    let fixture = Fixture::new();
    fixture.write(
        "backend/Cargo.toml",
        "[workspace]\nmembers = [\"crates/a\", \"crates/b\"]\nresolver = \"2\"\n",
    );
    fixture.package("backend/crates/a", "a", "");
    fixture.package("backend/crates/b", "b", "");
    let workspace = fixture.0.join("backend");
    assert!(package_dirs(&workspace).unwrap()["a"].contains(&"backend/crates/a/".into()));
    fixture.commit();
    let before = workspace_view(&workspace);
    assert_eq!(before.snapshot_id, snapshot(&workspace).unwrap());
    assert_eq!(before.snapshot_id, snapshot(&fixture.0).unwrap());

    fixture.write(
        "backend/crates/b/src/lib.rs",
        "// concurrent unrelated edit\n",
    );
    let unrelated = workspace_view(&workspace);
    assert!(!changed("cargo test -p a", &before, &unrelated));
    fixture.write("backend/crates/a/src/lib.rs", "// covered source\n");
    let first = workspace_view(&workspace);
    assert!(changed("cargo test -p a", &unrelated, &first));
    fixture.write(
        "backend/crates/a/src/lib.rs",
        "// same dirty path rewritten\n",
    );
    let second = workspace_view(&workspace);
    assert!(changed("cargo test -p a", &first, &second));
    assert_eq!(second.snapshot_id, snapshot(&workspace).unwrap());

    fixture.write(".cargo/config.toml", "[build]\njobs = 1\n");
    let ancestor_config = workspace_view(&workspace);
    assert!(changed("cargo test -p a", &second, &ancestor_config));
    fixture.write(
        "backend/Cargo.toml",
        "[workspace]\nmembers = [\"crates/a\", \"crates/b\"]\nresolver = \"2\"\n# changed\n",
    );
    let manifest = workspace_view(&workspace);
    assert!(changed("cargo test -p a", &ancestor_config, &manifest));
}

#[test]
fn staged_rename_out_of_a_package_retains_the_deleted_source_key() {
    let fixture = Fixture::new();
    fixture.write(
        "Cargo.toml",
        "[workspace]\nmembers = [\"crates/a\", \"crates/b\"]\nresolver = \"2\"\n",
    );
    fixture.package("crates/a", "a", "");
    fixture.package("crates/b", "b", "");
    fixture.write("crates/a/src/moved.rs", "pub fn moved() {}\n");
    assert!(package_dirs(&fixture.0).is_some());
    fixture.commit();
    let before = workspace_view(&fixture.0);
    fixture.git(&["mv", "crates/a/src/moved.rs", "crates/b/src/moved.rs"]);
    let after = workspace_view(&fixture.0);
    let diff = crate::daemon::diff_review::snapshot(&fixture.0).unwrap();
    assert!(
        diff.files
            .iter()
            .any(|file| file.old_path.as_deref() == Some("crates/a/src/moved.rs"))
    );
    let paths = after.dirty_fingerprints.as_ref().unwrap();
    assert!(paths.contains_key("crates/a/src/moved.rs"));
    assert!(paths.contains_key("crates/b/src/moved.rs"));
    assert!(changed("cargo test -p a", &before, &after));
    assert!(changed("cargo test -p b", &before, &after));
}

#[test]
fn target_aliases_union_same_named_targets_packages_and_lib_selector() {
    let fixture = Fixture::new();
    fixture.write("Cargo.toml", "[workspace]\nmembers = [\"crates/a\", \"crates/b\", \"crates/shared\", \"crates/lib\"]\nresolver = \"2\"\n");
    for name in ["a", "b"] {
        fixture.package(
            &format!("crates/{name}"),
            name,
            "[[test]]\nname = \"shared\"\npath = \"tests/shared.rs\"\n",
        );
        fixture.write(
            &format!("crates/{name}/tests/shared.rs"),
            "#[test] fn check() {}\n",
        );
    }
    fixture.package("crates/shared", "shared", "");
    fixture.package("crates/lib", "lib", "");
    let dirs = package_dirs(&fixture.0).unwrap();
    for name in ["a", "b", "shared"] {
        assert!(
            dirs["shared"].contains(&format!("crates/{name}/")),
            "missing {name}: {dirs:?}"
        );
    }
    for name in ["a", "b", "shared", "lib"] {
        assert!(dirs["lib"].contains(&format!("crates/{name}/")));
    }
    fixture.commit();
    let before = workspace_view(&fixture.0);
    for name in ["a", "b", "shared"] {
        fixture.write(&format!("crates/{name}/src/lib.rs"), "// changed\n");
        let after = workspace_view(&fixture.0);
        assert!(changed("cargo test --test shared", &before, &after));
        assert!(changed("cargo test -p shared", &before, &after));
    }
}

#[test]
fn local_dependency_custom_target_outside_package_is_covered() {
    let fixture = Fixture::new();
    fixture.write(
        "Cargo.toml",
        "[workspace]\nmembers = [\"crates/a\", \"crates/b\"]\nresolver = \"2\"\n",
    );
    fixture.package("crates/a", "a", "[lib]\npath = \"../../shared/lib.rs\"\n");
    fixture.write("shared/lib.rs", "pub fn value() {}\n");
    fixture.package("crates/b", "b", "[dependencies]\na = { path = \"../a\" }\n");
    let dirs = package_dirs(&fixture.0).unwrap();
    assert!(dirs["b"].contains(&"shared/".into()));
    fixture.commit();
    let before = workspace_view(&fixture.0);
    fixture.write("shared/lib.rs", "pub fn value() { /* changed */ }\n");
    assert!(changed(
        "cargo test -p b",
        &before,
        &workspace_view(&fixture.0)
    ));
}

#[test]
fn script_paths_are_entries_and_not_complete_dependency_sets() {
    let fixture = Fixture::new();
    fixture.write("checks.rb", "require_relative 'src/app'\n");
    fixture.write("src/app.rb", "VALUE = 1\n");
    fixture.write("tests/test_app.py", "import app\n");
    fixture.write("app.py", "VALUE = 1\n");
    fixture.commit();
    let before = workspace_view(&fixture.0);
    fixture.write("checks.rb", "require_relative 'src/app'\n# changed\n");
    let entry = workspace_view(&fixture.0);
    assert!(changed("ruby ./checks.rb", &before, &entry));
    fixture.write("src/app.rb", "VALUE = 2\n");
    fixture.write("app.py", "VALUE = 2\n");
    let sources = workspace_view(&fixture.0);
    for command in [
        "ruby checks.rb",
        "ruby ./checks.rb",
        "pytest tests/test_app.py",
    ] {
        assert!(changed(command, &entry, &sources), "{command}");
    }
}

#[tokio::test]
async fn script_mutations_reach_finish_and_never_pass_the_end_snapshot_gate() {
    use std::sync::{Arc, Mutex};
    use willdeep_core::tools::RunVerification;
    use willdeep_core::{ApprovalMode, ToolCall, ToolRegistry, VerificationStatus};

    for (path, content) in [
        ("checks.rb", "puts 'done'\n"),
        ("src/app.rb", "VALUE = 2\n"),
    ] {
        let fixture = Fixture::new();
        fixture.write("src/app.rb", "VALUE = 1\n");
        fixture.write(
            "checks.rb",
            &format!(
                "File.write('{path}', {})\n",
                serde_json::to_string(content).unwrap()
            ),
        );
        fixture.commit();
        let initial = snapshot(&fixture.0).unwrap();
        let capture_root = fixture.0.clone();
        let view_root = fixture.0.clone();
        let reports = Arc::new(Mutex::new(Vec::new()));
        let reported = reports.clone();
        let tools = ToolRegistry::new(&fixture.0, ApprovalMode::FullAccess)
            .unwrap()
            .with_fallible_verification_snapshot(move || snapshot(&capture_root))
            .with_verification_dirty_paths(move || workspace_view(&view_root))
            .with_verification_reporter(move |record| reported.lock().unwrap().push(record));
        let command = "ruby ./checks.rb";
        tools.require_verifications(&[command.into()]).unwrap();
        let call = ToolCall {
            id: "coverage-regression".into(),
            name: "run_command".into(),
            arguments: serde_json::json!({"command": command}).to_string(),
        };
        let output = tools.execute(&call).await.unwrap();
        assert!(output.contains("verification-invalidated"), "{output}");
        let current = snapshot(&fixture.0).unwrap();
        assert_ne!(initial, current);
        {
            let reports = reports.lock().unwrap();
            assert_eq!(reports.len(), 1);
            assert_eq!(reports[0].status, VerificationStatus::Invalidated);
            assert_eq!(reports[0].snapshot_id, initial);
            assert_eq!(reports[0].observed_snapshot_id, current);
        }
        assert_eq!(
            tools.run_verification(initial.as_deref(), 0),
            (RunVerification::Stale, 1)
        );
        tools.execute(&call).await.unwrap();
        assert_eq!(
            tools.run_verification(initial.as_deref(), 0),
            (RunVerification::Passed, 2)
        );
    }
}
