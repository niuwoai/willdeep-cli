use super::*;
use std::collections::BTreeMap;

fn assert_finish_evidence(
    command: &str,
    dirs: BTreeMap<String, Vec<String>>,
    before: BTreeMap<String, u64>,
    after: BTreeMap<String, u64>,
    expected: VerificationStatus,
) {
    let capture: VerificationSnapshot = Arc::new(|| Ok(Some("end".into())));
    let coverage = VerificationCoverage {
        dirty_before: Some(before),
        dirty_after: Some(after),
        view_id_before: Some("start".into()),
        view_id_after: Some("end".into()),
        head_before: Some(Some("head".into())),
        head_after: Some(Some("head".into())),
        package_dirs: Some(dirs),
    };
    let mut output = String::new();
    let finished = finish_verification(
        Some(&capture),
        command,
        &Some("start".into()),
        VerificationStatus::Passed,
        &mut output,
        Some(&coverage),
    );
    assert_eq!(finished.status, expected, "{command}: {output}");
    assert_eq!(finished.observed_snapshot_id.as_deref(), Some("end"));
    let mut records = EvidenceRecords::default();
    records.required.insert(command.into());
    records.record(CommandVerification {
        snapshot_id: Some("start".into()),
        observed_snapshot_id: finished.observed_snapshot_id,
        command: command.into(),
        exit_code: Some(0),
        status: finished.status,
        summary: output,
    });
    for revision in ["start", "end"] {
        assert_eq!(
            records.has_current_evidence(&Some(revision.into()), None),
            expected == VerificationStatus::Passed,
            "{command}: evidence for {revision}"
        );
    }
}

fn dirs(name: &str, roots: &[&str]) -> BTreeMap<String, Vec<String>> {
    BTreeMap::from([(
        name.into(),
        roots.iter().map(|root| (*root).into()).collect(),
    )])
}

fn fingerprints(paths: &[(&str, u64)]) -> BTreeMap<String, u64> {
    paths
        .iter()
        .map(|(path, hash)| ((*path).into(), *hash))
        .collect()
}

#[test]
fn root_package_source_changes_cannot_authorize_either_snapshot() {
    for (before, after) in [
        (BTreeMap::new(), fingerprints(&[("src/lib.rs", 1)])),
        (
            fingerprints(&[("src/lib.rs", 1)]),
            fingerprints(&[("src/lib.rs", 2)]),
        ),
        (fingerprints(&[("src/lib.rs", 1)]), BTreeMap::new()),
    ] {
        assert_finish_evidence(
            "cargo test -p app",
            dirs("app", &[""]),
            before,
            after,
            VerificationStatus::Invalidated,
        );
    }
}

#[test]
fn nested_workspace_uses_git_relative_coverage_at_finish() {
    assert_finish_evidence(
        "cargo test -p app",
        dirs("app", &["backend/crates/app/"]),
        BTreeMap::new(),
        fingerprints(&[("backend/crates/app/src/lib.rs", 1)]),
        VerificationStatus::Invalidated,
    );
}

#[test]
fn rename_out_of_selected_package_invalidates_the_end_snapshot() {
    assert_finish_evidence(
        "cargo test -p a",
        dirs("a", &["crates/a/"]),
        BTreeMap::new(),
        fingerprints(&[("crates/a/src/lib.rs", 1), ("crates/b/src/moved.rs", 2)]),
        VerificationStatus::Invalidated,
    );
}

#[test]
fn colliding_target_alias_covers_all_owners_at_finish() {
    for path in [
        "crates/a/tests/shared.rs",
        "crates/b/tests/shared.rs",
        "crates/shared/src/lib.rs",
    ] {
        assert_finish_evidence(
            "cargo test --test shared",
            dirs("shared", &["crates/a/", "crates/b/", "crates/shared/"]),
            BTreeMap::new(),
            fingerprints(&[(path, 1)]),
            VerificationStatus::Invalidated,
        );
    }
}

#[test]
fn scripts_cover_the_entry_and_business_sources_without_a_dependency_graph() {
    for command in [
        "ruby ./checks.rb",
        "ruby checks.rb",
        "pytest tests/test_app.py",
    ] {
        for path in ["checks.rb", "src/app.py", "tests/helper.py"] {
            assert_finish_evidence(
                command,
                BTreeMap::new(),
                BTreeMap::new(),
                fingerprints(&[(path, 1)]),
                VerificationStatus::Invalidated,
            );
        }
    }
}

#[test]
fn only_unambiguous_cargo_tests_can_reuse_coverage_for_an_unrelated_edit() {
    for command in [
        "cargo test -p app",
        "cargo test --package=app filter::name -- --nocapture",
        "cargo test -p app --features logging --locked",
        "cargo test -p app -- --package other",
    ] {
        assert_finish_evidence(
            command,
            dirs("app", &["crates/app/"]),
            BTreeMap::new(),
            fingerprints(&[("crates/other/src/lib.rs", 1)]),
            VerificationStatus::Passed,
        );
    }
    for command in [
        "pytest -p app tests/test_app.py",
        "ruby checks.rb --package app",
        "cargo build -p app",
        "cargo test -p app --manifest-path alternate/Cargo.toml",
        "cargo test -p app --config build.rustflags=[]",
        "cargo test -p app --target custom.json",
        "cargo test -p app --workspace",
        "cargo test -p app --all",
        "cargo test -p app --unknown-option",
        "cargo test -p app --package",
        "cargo test -p app --package=missing",
        "cargo test -p app --no-run",
        "cargo test -p app && true",
        "cargo test -- --package app",
    ] {
        assert_finish_evidence(
            command,
            dirs("app", &["crates/app/"]),
            BTreeMap::new(),
            fingerprints(&[("crates/other/src/lib.rs", 1)]),
            VerificationStatus::Invalidated,
        );
    }
}

#[test]
fn changed_or_missing_metadata_cannot_supply_finish_coverage() {
    for after_dirs in [None, Some(dirs("app", &["alternate/"]))] {
        let before = VerificationWorkspaceView {
            snapshot_id: Some("start".into()),
            head: Some(Some("head".into())),
            dirty_fingerprints: Some(BTreeMap::new()),
            package_dirs: Some(dirs("app", &["crates/app/"])),
        };
        let after = VerificationWorkspaceView {
            snapshot_id: Some("end".into()),
            head: Some(Some("head".into())),
            dirty_fingerprints: Some(fingerprints(&[("crates/other/src/lib.rs", 1)])),
            package_dirs: after_dirs,
        };
        let coverage = verification_coverage(Some(before), Some(after));
        assert!(coverage.package_dirs.is_none());
        let capture: VerificationSnapshot = Arc::new(|| Ok(Some("end".into())));
        let finished = finish_verification(
            Some(&capture),
            "cargo test -p app",
            &Some("start".into()),
            VerificationStatus::Passed,
            &mut String::new(),
            Some(&coverage),
        );
        assert_eq!(finished.status, VerificationStatus::Invalidated);
    }
}
