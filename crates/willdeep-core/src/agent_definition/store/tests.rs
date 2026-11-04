use super::*;
use serde_json::json;
use std::sync::{Arc, Barrier};

struct Fixture(PathBuf, AgentLibrary);
impl Fixture {
    fn new() -> Self {
        let home = std::env::temp_dir().join(format!("agent-cas-{}", uuid::Uuid::new_v4()));
        Self(home.clone(), AgentLibrary::new(&home))
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn package(version: &str) -> AgentPackage {
    let mut definition = super::super::AgentDefinition::parse(include_bytes!(
        "../../../../../docs/examples/agent-definition.v1.json"
    ))
    .unwrap();
    definition.version = version.into();
    AgentPackage::local(definition).unwrap()
}
fn request(key: &str, expected: Option<u64>) -> MutationRequest {
    MutationRequest {
        request_id: key.into(),
        expected_revision: expected,
        payload: json!({"scope":"test"}),
    }
}
fn install(
    library: &AgentLibrary,
    package: &AgentPackage,
    key: &str,
    expected: Option<u64>,
) -> Result<MutationReceipt> {
    library.mutate(
        &request(key, expected),
        || Ok(()),
        Mutation::Install {
            package,
            origin: Origin::Local,
            keys: &BTreeMap::new(),
            binding: None,
            compatible: true,
            create_only: expected.is_none(),
            allow_unchanged: false,
        },
    )
}
fn revision(library: &AgentLibrary, id: &str) -> u64 {
    library.installations().unwrap()[id].installation_revision
}

#[test]
fn concurrent_create_has_one_winner_and_one_atomic_state() {
    let fixture = Fixture::new();
    let package = Arc::new(package("1.0.0"));
    let barrier = Arc::new(Barrier::new(3));
    let joins: Vec<_> = (0..2)
        .map(|index| {
            let library = fixture.1.clone();
            let package = package.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                install(&library, &package, &format!("create-{index}"), None).is_ok()
            })
        })
        .collect();
    barrier.wait();
    assert_eq!(
        joins
            .into_iter()
            .map(|join| usize::from(join.join().unwrap()))
            .sum::<usize>(),
        1
    );
    let state = fixture.1.state().unwrap();
    assert_eq!(state.installations.len(), 1);
    assert_eq!(state.clock, 1);
    assert_eq!(state.receipts.len(), 1);
}

#[test]
fn all_installation_mutations_use_cas_and_advance_even_for_noops() {
    let fixture = Fixture::new();
    let library = &fixture.1;
    let first = package("1.0.0");
    let id = &first.definition.definition_id;
    let initial = install(library, &first, "create", None)
        .unwrap()
        .installation
        .installation_revision;
    let binding = ModelBinding {
        model: Some("fixture-model".into()),
        context_window: Some(8192),
        ..Default::default()
    };
    let mutations = [
        Mutation::Bind { id, binding },
        Mutation::Enable { id, value: false },
        Mutation::Enable { id, value: false },
        Mutation::Pin { id, value: true },
        Mutation::Rollback {
            id,
            revision: &first.sha256,
        },
        Mutation::Pin { id, value: false },
    ];
    for (index, mutation) in mutations.into_iter().enumerate() {
        let previous = revision(library, id);
        let receipt = library
            .mutate(
                &request(&format!("write-{index}"), Some(previous)),
                || Ok(()),
                mutation,
            )
            .unwrap();
        assert!(receipt.installation.installation_revision > previous);
        assert_eq!(receipt.installation.sha256, first.sha256);
        assert!(
            library
                .mutate(
                    &request(&format!("stale-{index}"), Some(initial)),
                    || Ok(()),
                    Mutation::Enable { id, value: true }
                )
                .unwrap_err()
                .to_string()
                .contains("agent_revision_conflict")
        );
    }
    assert!(
        library
            .mutate(
                &request("missing", None),
                || Ok(()),
                Mutation::Pin { id, value: false }
            )
            .unwrap_err()
            .to_string()
            .contains("agent_revision_required")
    );
    let second = package("1.0.0-rc2");
    assert!(
        install(library, &second, "body-stale", Some(initial))
            .unwrap_err()
            .to_string()
            .contains("agent_revision_conflict")
    );
    install(library, &second, "body", Some(revision(library, id))).unwrap();
}

#[test]
fn request_replay_survives_later_changes_and_payload_or_operation_reuse_conflicts() {
    let fixture = Fixture::new();
    let library = &fixture.1;
    let first = package("1.0.0");
    let id = &first.definition.definition_id;
    let created = install(library, &first, "create", None).unwrap();
    library
        .mutate(
            &request("disable", Some(created.installation.installation_revision)),
            || Ok(()),
            Mutation::Enable { id, value: false },
        )
        .unwrap();
    let replay = install(library, &first, "create", None).unwrap();
    assert!(replay.replayed);
    assert_eq!(replay.installation, created.installation);
    let mut changed = request("create", None);
    changed.payload = json!({"scope":"changed"});
    assert!(
        library
            .mutate(&changed, || Ok(()), Mutation::Pin { id, value: false })
            .unwrap_err()
            .to_string()
            .contains("agent_request_conflict")
    );
    // A caller cannot hide different semantics behind an unchanged payload.
    assert!(
        library
            .mutate(
                &request("create", None),
                || Ok(()),
                Mutation::Pin { id, value: false }
            )
            .unwrap_err()
            .to_string()
            .contains("agent_request_conflict")
    );
    assert_eq!(library.state().unwrap().receipts.len(), 2);
}

#[test]
fn scope_validation_runs_under_the_shared_account_lock_including_replays() {
    let fixture = Fixture::new();
    let local = &fixture.1;
    let account = local.account_scope("fixture-account");
    let first = package("1.0.0");
    install(local, &first, "create", None).unwrap();
    let lock = account.lock_scope().unwrap();
    let (sender, receiver) = std::sync::mpsc::channel();
    let library = local.clone();
    let package = first.clone();
    let thread = std::thread::spawn(move || {
        library.mutate(
            &request("create", None),
            || {
                sender.send(()).unwrap();
                anyhow::bail!("agent_scope_conflict: synthetic account change")
            },
            Mutation::Install {
                package: &package,
                origin: Origin::Local,
                keys: &BTreeMap::new(),
                binding: None,
                compatible: true,
                create_only: true,
                allow_unchanged: false,
            },
        )
    });
    assert!(
        receiver
            .recv_timeout(std::time::Duration::from_millis(50))
            .is_err()
    );
    drop(lock);
    receiver
        .recv_timeout(std::time::Duration::from_secs(2))
        .unwrap();
    assert!(
        thread
            .join()
            .unwrap()
            .unwrap_err()
            .to_string()
            .contains("agent_scope_conflict")
    );
    assert_eq!(local.state().unwrap().clock, 1);
}

#[test]
fn legacy_map_migrates_only_on_successful_write_and_preserves_receipts() {
    let fixture = Fixture::new();
    let library = &fixture.1;
    let first = package("1.0.0");
    let id = &first.definition.definition_id;
    library
        .install(&first, Origin::Local, &BTreeMap::new())
        .unwrap();
    let mut legacy = serde_json::to_value(library.installations().unwrap()).unwrap();
    legacy[id]
        .as_object_mut()
        .unwrap()
        .remove("installation_revision");
    let path = library.root().join("installations.json");
    private_write(&path, &serde_json::to_vec(&legacy).unwrap()).unwrap();
    assert_eq!(revision(library, id), 1);
    assert!(
        serde_json::from_slice::<serde_json::Value>(&fs::read(&path).unwrap())
            .unwrap()
            .get("schema")
            .is_none()
    );
    library
        .mutate(
            &request("disable", Some(1)),
            || Ok(()),
            Mutation::Enable { id, value: false },
        )
        .unwrap();
    let value: serde_json::Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
    assert_eq!(value["schema"], "willdeep.agent-installations.v2");
    assert_eq!(value["clock"], 2);
    assert!(value["receipts"].get("disable").is_some());
    library.set_binding(id, ModelBinding::default()).unwrap();
    assert!(library.state().unwrap().receipts.contains_key("disable"));
}

#[test]
fn delete_recreate_and_rollback_do_not_allow_aba() {
    let fixture = Fixture::new();
    let library = &fixture.1;
    let first = package("1.0.0");
    let id = &first.definition.definition_id;
    let initial = install(library, &first, "first", None)
        .unwrap()
        .installation
        .installation_revision;
    let second = package("1.0.0-rc2");
    install(library, &second, "second", Some(initial)).unwrap();
    library
        .mutate(
            &request("rollback", Some(revision(library, id))),
            || Ok(()),
            Mutation::Rollback {
                id,
                revision: &first.sha256,
            },
        )
        .unwrap();
    assert!(revision(library, id) > initial);
    library.remove(id, &first.sha256).unwrap();
    install(library, &first, "recreate", None).unwrap();
    assert!(revision(library, id) > initial);
    assert!(
        library
            .mutate(
                &request("stale", Some(initial)),
                || Ok(()),
                Mutation::Enable { id, value: true }
            )
            .is_err()
    );
    library.set_pinned(id, false).unwrap();
    assert!(
        install(
            library,
            &first,
            "reuse-old-body",
            Some(revision(library, id))
        )
        .is_err()
    );
}

#[test]
fn immutable_versions_are_strictly_increasing_with_numeric_rc_order() {
    let fixture = Fixture::new();
    let library = &fixture.1;
    let first = package("1.0.0-rc9");
    let id = &first.definition.definition_id;
    install(library, &first, "first", None).unwrap();
    let mut different = first.definition.clone();
    different.instructions.push_str(" changed");
    let different = AgentPackage::local(different).unwrap();
    assert!(
        install(
            library,
            &different,
            "collision",
            Some(revision(library, id))
        )
        .unwrap_err()
        .to_string()
        .contains("same agent version")
    );
    let older = package("1.0.0-rc8");
    assert!(install(library, &older, "older", Some(revision(library, id))).is_err());
    let newer = package("1.0.0-rc10");
    install(library, &newer, "newer", Some(revision(library, id))).unwrap();
    assert!(install(library, &newer, "same", Some(revision(library, id))).is_err());
    assert_eq!(library.history(id).unwrap().len(), 2);
}

#[test]
fn fork_checks_source_cas_destination_absence_and_replays_target_identity() {
    let fixture = Fixture::new();
    let local = &fixture.1;
    let source = local.account_scope("fixture-account");
    let first = package("1.0.0");
    let id = &first.definition.definition_id;
    install(&source, &first, "source", None).unwrap();
    let revision = revision(&source, id);
    let request = request("fork", Some(revision));
    let fork = || Mutation::Fork {
        source: &source,
        id,
        new_id: "user/fork",
    };
    let receipt = local.mutate(&request, || Ok(()), fork()).unwrap();
    assert_eq!(receipt.installation.definition_id, "user/fork");
    let package = local.package(&receipt.installation.sha256).unwrap();
    assert_eq!(package.definition.forked_from, Some(first.sha256));
    assert!(local.mutate(&request, || Ok(()), fork()).unwrap().replayed);
    assert!(
        local
            .mutate(
                &super::tests::request("duplicate-fork", Some(revision)),
                || Ok(()),
                fork()
            )
            .is_err()
    );
    assert!(
        local
            .mutate(
                &super::tests::request("stale-fork", Some(revision + 1)),
                || Ok(()),
                Mutation::Fork {
                    source: &source,
                    id,
                    new_id: "user/other"
                }
            )
            .is_err()
    );
}

#[test]
fn invalid_binding_and_foreign_rollback_do_not_commit() {
    let fixture = Fixture::new();
    let library = &fixture.1;
    let first = package("1.0.0");
    let id = &first.definition.definition_id;
    install(library, &first, "first", None).unwrap();
    let before = fs::read(library.root().join("installations.json")).unwrap();
    let invalid = ModelBinding {
        max_turns: Some(0),
        ..Default::default()
    };
    assert!(
        library
            .mutate(
                &request("bad-binding", Some(1)),
                || Ok(()),
                Mutation::Bind {
                    id,
                    binding: invalid
                }
            )
            .is_err()
    );
    let mut other = first.definition.clone();
    other.definition_id = "user/other".into();
    let other = AgentPackage::local(other).unwrap();
    library.archive_private("user/other", &other).unwrap();
    assert!(
        library
            .mutate(
                &request("bad-rollback", Some(1)),
                || Ok(()),
                Mutation::Rollback {
                    id,
                    revision: &other.sha256
                }
            )
            .is_err()
    );
    assert_eq!(
        fs::read(library.root().join("installations.json")).unwrap(),
        before
    );
}
