use super::*;
use serde_json::json;

struct Fixture {
    home: PathBuf,
    store: WorkflowStore,
    run: WorkflowRun,
}
impl Fixture {
    fn new() -> Self {
        let home = std::env::temp_dir().join(format!("workflow-host-snapshot-{}", Uuid::new_v4()));
        let store = WorkflowStore::new(&home, Uuid::new_v4());
        let spec = example_spec();
        store.save_definition(&spec).unwrap();
        let run = store
            .start(&spec.id, Value::Null, WorkflowLimits::default())
            .unwrap();
        Self { home, store, run }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.home).unwrap();
    }
}

#[test]
fn host_snapshot_is_private_durable_write_once_and_does_not_bump_revision() {
    let f = Fixture::new();
    assert_eq!(f.store.host_snapshot(f.run.id).unwrap(), None);
    let saved = json!({"version":1,"profiles":{"fixture":{"model":"frozen"}}});
    f.store.pin_host_snapshot(f.run.id, saved.clone()).unwrap();
    let reopened = WorkflowStore::new(&f.home, f.run.parent_session_id);
    assert_eq!(
        reopened.host_snapshot(f.run.id).unwrap(),
        Some(saved.clone())
    );
    reopened.pin_host_snapshot(f.run.id, saved.clone()).unwrap();
    assert_eq!(reopened.get(f.run.id).unwrap(), Some(f.run.clone()));
    let public = serde_json::to_value(reopened.get(f.run.id).unwrap().unwrap()).unwrap();
    assert!(public.get("host_snapshot").is_none());
    assert!(!serde_json::to_string(&public).unwrap().contains("frozen"));
    assert!(matches!(
        reopened.pin_host_snapshot(f.run.id, json!({"different":true})),
        Err(WorkflowError::State(_))
    ));
    assert_eq!(reopened.host_snapshot(f.run.id).unwrap(), Some(saved));
    assert!(reopened.host_snapshot(Uuid::new_v4()).is_err());
    assert!(
        reopened
            .pin_host_snapshot(Uuid::new_v4(), Value::Null)
            .is_err()
    );
    let first = reopened.poll(f.run.id).unwrap();
    let second = reopened.poll(f.run.id).unwrap();
    assert_eq!(first.run.revision, second.run.revision);
    assert!(first.run.revision > f.run.revision);
}

#[test]
fn host_snapshot_retry_and_resume_reuse_source_and_keep_replay_identity() {
    let f = Fixture::new();
    let saved = json!({"original":"catalog"});
    f.store.pin_host_snapshot(f.run.id, saved.clone()).unwrap();
    f.store.stop(f.run.id).unwrap();
    assert!(f.store.poll(f.run.id).unwrap().run.status.is_terminal());
    for retry in [true, false] {
        let request = Uuid::new_v4();
        let restarted = if retry {
            f.store.retry_with_request(f.run.id, request).unwrap()
        } else {
            f.store.resume_with_request(f.run.id, request).unwrap()
        };
        assert_eq!(restarted.source_run_id, Some(f.run.id));
        assert_eq!(restarted.revision, 1);
        assert_eq!(
            f.store.host_snapshot(restarted.id).unwrap(),
            Some(saved.clone())
        );
        f.store
            .pin_host_snapshot(restarted.id, saved.clone())
            .unwrap();
        assert!(
            f.store
                .pin_host_snapshot(restarted.id, json!({"new":"catalog"}))
                .is_err()
        );
        let replay = if retry {
            f.store.retry_with_request(f.run.id, request).unwrap()
        } else {
            f.store.resume_with_request(f.run.id, request).unwrap()
        };
        assert_eq!(replay, restarted);
    }
}

#[test]
fn host_snapshot_pinning_is_atomic_with_competing_hosts() {
    let f = Fixture::new();
    let start = std::sync::Arc::new(std::sync::Barrier::new(3));
    let mut hosts = Vec::new();
    for contender in ["first", "second"] {
        let store = f.store.clone();
        let start = start.clone();
        let id = f.run.id;
        hosts.push(std::thread::spawn(move || {
            start.wait();
            let snapshot = json!({"contender":contender});
            let result = store.pin_host_snapshot(id, snapshot.clone());
            (snapshot, result)
        }));
    }
    start.wait();
    let results: Vec<_> = hosts.into_iter().map(|host| host.join().unwrap()).collect();
    assert_eq!(
        results.iter().filter(|(_, result)| result.is_ok()).count(),
        1
    );
    let winner = results
        .into_iter()
        .find(|(_, result)| result.is_ok())
        .unwrap()
        .0;
    assert_eq!(f.store.host_snapshot(f.run.id).unwrap(), Some(winner));
    assert_eq!(
        f.store.get(f.run.id).unwrap().unwrap().revision,
        f.run.revision
    );
}

#[test]
fn host_snapshot_bounds_and_failed_commit_leave_run_unpinned() {
    let f = Fixture::new();
    assert!(matches!(
        f.store
            .pin_host_snapshot(f.run.id, json!("x".repeat(MAX_HOST_SNAPSHOT_BYTES))),
        Err(WorkflowError::Limit(_))
    ));
    assert_eq!(f.store.host_snapshot(f.run.id).unwrap(), None);
    let mut deep = Value::Null;
    for _ in 0..=WORKFLOW_MAX_JSON_DEPTH {
        deep = json!([deep]);
    }
    assert!(matches!(
        f.store.pin_host_snapshot(f.run.id, deep),
        Err(WorkflowError::Limit(_))
    ));
    // The host ceiling does not bypass the run's smaller total-byte admission.
    let mut record = f.store.required_run(f.run.id).unwrap();
    record.run.limits.max_run_bytes = serde_json::to_vec(&record).unwrap().len() + 128;
    f.store.write_run(&record).unwrap();
    assert!(matches!(
        f.store.pin_host_snapshot(f.run.id, json!("x".repeat(1024))),
        Err(WorkflowError::Limit(_))
    ));
    assert_eq!(f.store.host_snapshot(f.run.id).unwrap(), None);
    assert_eq!(f.store.get(f.run.id).unwrap().unwrap(), record.run);
}

#[test]
fn host_snapshot_null_is_pinned_and_legacy_missing_field_remains_readable() {
    let f = Fixture::new();
    let original = f.store.required_run(f.run.id).unwrap();
    let mut legacy = serde_json::to_value(&original).unwrap();
    legacy.as_object_mut().unwrap().remove("host_snapshot");
    let decoded: StoredRun = serde_json::from_value(legacy).unwrap();
    assert!(decoded.host_snapshot.is_none());
    f.store.pin_host_snapshot(f.run.id, Value::Null).unwrap();
    assert_eq!(f.store.host_snapshot(f.run.id).unwrap(), Some(Value::Null));
    f.store.pin_host_snapshot(f.run.id, Value::Null).unwrap();
    assert!(f.store.pin_host_snapshot(f.run.id, json!({})).is_err());
}
