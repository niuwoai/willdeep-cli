use super::*;
use crate::tools::{ApprovalDecision, ApprovalMode, Approver};
use std::path::PathBuf;
use std::sync::{Arc, Barrier, mpsc};
use std::time::Duration;

const WAIT: Duration = Duration::from_secs(5);

fn assert_durable(receipt: PersistenceReceipt) {
    assert!(
        matches!(&receipt, PersistenceReceipt::Durable),
        "{receipt:?}"
    );
}

struct Fixture {
    root: PathBuf,
    store: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let root =
            std::env::temp_dir().join(format!("willdeep-always-allow-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        Self {
            store: root.join("always-allow.json"),
            root,
        }
    }

    fn registry(&self) -> ToolRegistry {
        ToolRegistry::new(&self.root, ApprovalMode::Strict)
            .unwrap()
            .with_always_allow_store(self.store.clone())
            .unwrap()
    }

    fn seed(&self, rules: &[&str]) {
        let store = LockedStore::acquire(&self.store).unwrap();
        let bytes = serde_json::to_vec(rules).unwrap();
        assert_durable(atomic_replace(&self.store, |file| file.write_all(&bytes)).unwrap());
        drop(store);
    }

    fn rules(&self) -> HashSet<String> {
        serde_json::from_slice::<Vec<String>>(&std::fs::read(&self.store).unwrap())
            .unwrap()
            .into_iter()
            .collect()
    }

    fn temporaries(&self) -> Vec<PathBuf> {
        std::fs::read_dir(&self.root)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| path.extension().is_some_and(|extension| extension == "tmp"))
            .collect()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn approve_in_thread(
    registry: Arc<ToolRegistry>,
    signature: &str,
) -> std::thread::JoinHandle<Result<(), ToolError>> {
    let signature = signature.to_owned();
    std::thread::spawn(move || {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(registry.require_rememberable_approval("fixture action", signature))
    })
}

fn persist_in_runtime(
    registry: &ToolRegistry,
    signature: impl Into<String>,
) -> Result<(), ToolError> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(registry.persist_always_allowed(signature.into()))
        .map(PersistenceReceipt::warn_if_unconfirmed)
}

#[test]
fn concurrent_registries_merge_their_explicit_additions() {
    let fixture = Fixture::new();
    fixture.seed(&["command-exact:existing"]);
    const WRITERS: usize = 8;
    let barrier = Arc::new(Barrier::new(WRITERS));
    // Every registry starts from the same old snapshot, before any writer runs.
    let registries: Vec<_> = (0..WRITERS).map(|_| fixture.registry()).collect();
    let writers: Vec<_> = registries
        .into_iter()
        .enumerate()
        .map(|(index, registry)| {
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                let signature = format!("command-exact:fixture --writer={index}");
                barrier.wait();
                persist_in_runtime(&registry, signature.clone()).unwrap();
                assert!(registry.always_allowed.lock().unwrap().contains(&signature));
            })
        })
        .collect();
    for writer in writers {
        writer.join().unwrap();
    }
    let rules = fixture.rules();
    assert_eq!(rules.len(), WRITERS + 1);
    assert!(rules.contains("command-exact:existing"));
    for index in 0..WRITERS {
        assert!(rules.contains(&format!("command-exact:fixture --writer={index}")));
    }
    assert_eq!(*fixture.registry().always_allowed.lock().unwrap(), rules);
    assert!(fixture.temporaries().is_empty());
}

#[test]
fn concurrent_commits_publish_the_latest_union_to_shared_memory() {
    let fixture = Fixture::new();
    let registry = Arc::new(fixture.registry());
    const WRITERS: usize = 8;
    let barrier = Arc::new(Barrier::new(WRITERS));
    let writers: Vec<_> = (0..WRITERS)
        .map(|index| {
            let registry = registry.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                persist_in_runtime(&registry, format!("command-exact:shared --writer={index}"))
                    .unwrap();
            })
        })
        .collect();
    for writer in writers {
        writer.join().unwrap();
    }
    let rules = fixture.rules();
    assert_eq!(rules.len(), WRITERS);
    assert_eq!(*registry.always_allowed.lock().unwrap(), rules);
}

struct AnnouncingApprover(mpsc::Sender<()>);

#[async_trait::async_trait]
impl Approver for AnnouncingApprover {
    async fn approve(&self, _description: &str, always_allow_available: bool) -> ApprovalDecision {
        assert!(always_allow_available);
        self.0.send(()).unwrap();
        ApprovalDecision::AlwaysAllow
    }
}

// A separate thread releases the lock even if a regression blocks the sole
// runtime thread, so the cancellation/timeout tests fail instead of hanging.
fn hold_disk_lock(path: PathBuf) -> (mpsc::Sender<()>, std::thread::JoinHandle<bool>) {
    let (ready, acquired) = mpsc::channel();
    let (release, released) = mpsc::channel();
    let holder = std::thread::spawn(move || {
        let store = LockedStore::acquire(&path).unwrap();
        ready.send(()).unwrap();
        let explicitly_released = released.recv_timeout(WAIT).is_ok();
        drop(store);
        explicitly_released
    });
    acquired.recv_timeout(WAIT).unwrap();
    (release, holder)
}

#[tokio::test(flavor = "current_thread")]
async fn cancelling_a_contended_commit_keeps_the_runtime_responsive_and_never_commits() {
    let fixture = Fixture::new();
    fixture.seed(&["command-exact:old"]);
    let (entered, requests) = mpsc::channel();
    let registry = Arc::new(
        fixture
            .registry()
            .with_approver(Arc::new(AnnouncingApprover(entered))),
    );
    let (release, holder) = hold_disk_lock(fixture.store.clone());
    let requesting_registry = registry.clone();
    let request = tokio::spawn(async move {
        requesting_registry
            .require_rememberable_approval("pending fixture action", "command-exact:new".to_owned())
            .await
    });
    // This timer and the abort must run while the disk lock is still held.
    tokio::time::sleep(LOCK_RETRY_INTERVAL * 2).await;
    let asked = requests.try_recv();
    let completed_before_abort = request.is_finished();
    request.abort();
    let cancelled = matches!(request.await, Err(error) if error.is_cancelled());
    let requested_release = release.send(()).is_ok();
    let released_before_watchdog = holder.join().unwrap();
    // Catch a detached writer that would resume after cancellation and unlock.
    tokio::time::sleep(LOCK_RETRY_INTERVAL * 2).await;
    assert!(asked.is_ok());
    assert!(!completed_before_abort);
    assert!(cancelled);
    assert!(requested_release && released_before_watchdog);
    let expected = HashSet::from(["command-exact:old".to_owned()]);
    assert_eq!(fixture.rules(), expected);
    assert_eq!(*registry.always_allowed.lock().unwrap(), expected);
    assert_eq!(*fixture.registry().always_allowed.lock().unwrap(), expected);
    assert!(fixture.temporaries().is_empty());
}

#[tokio::test(flavor = "current_thread")]
async fn a_contended_async_store_lock_has_a_bounded_wait() {
    let fixture = Fixture::new();
    fixture.seed(&["command-exact:old"]);
    let (release, holder) = hold_disk_lock(fixture.store.clone());
    let error = LockedStore::acquire_async(&fixture.store, LOCK_RETRY_INTERVAL * 2)
        .await
        .err();
    let requested_release = release.send(()).is_ok();
    let released_before_watchdog = holder.join().unwrap();
    assert!(requested_release && released_before_watchdog);
    let error = error.expect("contention must time out before the lock is released");
    assert_eq!(error.kind(), io::ErrorKind::TimedOut);
    assert!(error.to_string().contains("not committed"));
    assert_eq!(
        fixture.rules(),
        HashSet::from(["command-exact:old".to_owned()])
    );
    assert!(fixture.temporaries().is_empty());
}

#[tokio::test(flavor = "current_thread")]
async fn timing_out_approval_during_lock_contention_never_commits_after_unlock() {
    let fixture = Fixture::new();
    fixture.seed(&["command-exact:old"]);
    let (entered, requests) = mpsc::channel();
    let registry = fixture
        .registry()
        .with_approver(Arc::new(AnnouncingApprover(entered)));
    let (release, holder) = hold_disk_lock(fixture.store.clone());
    let result = tokio::time::timeout(
        LOCK_RETRY_INTERVAL * 2,
        registry.require_rememberable_approval(
            "pending fixture action",
            "command-exact:new".to_owned(),
        ),
    )
    .await;
    let asked = requests.try_recv();
    let memory_before_unlock = registry.always_allowed.lock().unwrap().clone();
    let requested_release = release.send(()).is_ok();
    let released_before_watchdog = holder.join().unwrap();
    // An outer timeout must drop the waiter, not leave a background commit.
    tokio::time::sleep(LOCK_RETRY_INTERVAL * 2).await;
    assert!(
        result.is_err(),
        "the caller's timeout must expire while the lock is held"
    );
    assert!(asked.is_ok());
    assert!(requested_release && released_before_watchdog);
    let expected = HashSet::from(["command-exact:old".to_owned()]);
    assert_eq!(memory_before_unlock, expected);
    assert_eq!(fixture.rules(), expected);
    assert_eq!(*registry.always_allowed.lock().unwrap(), expected);
    assert_eq!(*fixture.registry().always_allowed.lock().unwrap(), expected);
    assert!(fixture.temporaries().is_empty());
}

#[test]
fn a_commit_waiting_for_the_disk_lock_does_not_allow_a_second_call() {
    let fixture = Fixture::new();
    fixture.seed(&["command-exact:old"]);
    let (entered, requests) = mpsc::channel();
    let registry = Arc::new(
        fixture
            .registry()
            .with_approver(Arc::new(AnnouncingApprover(entered))),
    );
    let store = LockedStore::acquire(&fixture.store).unwrap();
    let first = approve_in_thread(registry.clone(), "command-exact:new");
    let first_asked = requests.recv_timeout(WAIT);
    let second = approve_in_thread(registry.clone(), "command-exact:new");
    let second_asked = requests.recv_timeout(WAIT);
    let memory_before_commit = registry
        .always_allowed
        .try_lock()
        .ok()
        .map(|rules| rules.clone());
    let first_finished = first.is_finished();
    let second_finished = second.is_finished();
    let disk_before_commit = fixture.rules();
    // Always release before asserting or joining, including on a regression.
    drop(store);
    let first_result = first.join().unwrap();
    let second_result = second.join().unwrap();
    assert!(first_asked.is_ok());
    assert!(second_asked.is_ok(), "the second call must still ask");
    assert!(!first_finished && !second_finished);
    assert_eq!(
        memory_before_commit,
        Some(HashSet::from(["command-exact:old".to_owned()]))
    );
    assert_eq!(
        disk_before_commit,
        HashSet::from(["command-exact:old".to_owned()])
    );
    first_result.unwrap();
    second_result.unwrap();
    assert_eq!(fixture.rules().len(), 2);
    assert!(
        registry
            .always_allowed
            .lock()
            .unwrap()
            .contains("command-exact:new")
    );
}

struct SuspendedApprover {
    entered: tokio::sync::mpsc::UnboundedSender<()>,
    resume: tokio::sync::Semaphore,
}

#[async_trait::async_trait]
impl Approver for SuspendedApprover {
    async fn approve(&self, _description: &str, always_allow_available: bool) -> ApprovalDecision {
        assert!(always_allow_available);
        self.entered.send(()).unwrap();
        self.resume.acquire().await.unwrap().forget();
        ApprovalDecision::AlwaysAllow
    }
}

#[tokio::test]
async fn approval_await_holds_no_memory_lock_or_uncommitted_permission() {
    for persistent in [false, true] {
        let fixture = Fixture::new();
        let (entered, mut requests) = tokio::sync::mpsc::unbounded_channel();
        let approver = Arc::new(SuspendedApprover {
            entered,
            resume: tokio::sync::Semaphore::new(0),
        });
        let registry = ToolRegistry::new(&fixture.root, ApprovalMode::Strict)
            .unwrap()
            .with_approver(approver.clone());
        let registry = Arc::new(if persistent {
            registry
                .with_always_allow_store(fixture.store.clone())
                .unwrap()
        } else {
            registry
        });
        let spawn_request = |registry: Arc<ToolRegistry>| {
            tokio::spawn(async move {
                registry
                    .require_rememberable_approval(
                        "pending action",
                        "command-exact:pending".to_owned(),
                    )
                    .await
            })
        };
        let first = spawn_request(registry.clone());
        assert_eq!(
            tokio::time::timeout(WAIT, requests.recv()).await.unwrap(),
            Some(())
        );
        assert!(registry.always_allowed.try_lock().unwrap().is_empty());
        // Use try_lock so a lock held across the approval await fails promptly.
        if persistent {
            let store = private_options()
                .read(true)
                .open(fixture.store.with_extension("json.lock"))
                .unwrap();
            store
                .try_lock()
                .expect("no store lock while awaiting approval");
            drop(store);
        }
        let second = spawn_request(registry.clone());
        assert_eq!(
            tokio::time::timeout(WAIT, requests.recv()).await.unwrap(),
            Some(())
        );
        assert!(registry.always_allowed.try_lock().unwrap().is_empty());
        approver.resume.add_permits(2);
        first.await.unwrap().unwrap();
        second.await.unwrap().unwrap();
        assert!(
            registry
                .always_allowed
                .lock()
                .unwrap()
                .contains("command-exact:pending")
        );
        if persistent {
            assert_eq!(
                fixture.rules(),
                HashSet::from(["command-exact:pending".to_owned()])
            );
        }
    }
}

#[test]
fn partial_temporary_write_failure_preserves_the_complete_old_file() {
    let fixture = Fixture::new();
    fixture.seed(&["command-exact:old", "mcp:existing"]);
    let original = std::fs::read(&fixture.store).unwrap();
    let store = LockedStore::acquire(&fixture.store).unwrap();
    let error = atomic_replace(&fixture.store, |file| {
        file.write_all(b"[\"command-exact:partial")?;
        Err(io::Error::new(
            io::ErrorKind::StorageFull,
            "synthetic write failure",
        ))
    })
    .unwrap_err();
    drop(store);
    assert_eq!(error.kind(), io::ErrorKind::StorageFull);
    assert_eq!(std::fs::read(&fixture.store).unwrap(), original);
    assert!(fixture.temporaries().is_empty());
    assert_eq!(fixture.rules().len(), 2);
}

#[test]
fn directory_sync_preflight_failure_does_not_replace_or_publish() {
    let fixture = Fixture::new();
    fixture.seed(&["command-exact:old"]);
    let registry = fixture.registry();
    let store = LockedStore::acquire(&fixture.store).unwrap();
    let original = std::fs::read(&fixture.store).unwrap();
    let mut write_called = false;
    let result = store.update_with_replace(Some("command-exact:new"), |path, bytes| {
        atomic_replace_with_sync(
            path,
            |file| {
                write_called = true;
                file.write_all(bytes)
            },
            || {
                Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "synthetic directory sync failure",
                ))
            },
        )
    });
    drop(store);
    let error = result.err().expect("preflight must fail before writing");
    assert!(
        matches!(error, ToolError::Io(ref error) if error.kind() == io::ErrorKind::PermissionDenied)
    );
    assert!(error.to_string().contains("not committed"));
    assert!(!write_called);
    assert_eq!(std::fs::read(&fixture.store).unwrap(), original);
    assert_eq!(
        *registry.always_allowed.lock().unwrap(),
        HashSet::from(["command-exact:old".to_owned()])
    );
    assert!(fixture.temporaries().is_empty());
}

#[test]
fn post_commit_directory_sync_failure_publishes_and_reports_the_active_rule() {
    let fixture = Fixture::new();
    fixture.seed(&["command-exact:old"]);
    let registry = fixture.registry();
    let store = LockedStore::acquire(&fixture.store).unwrap();
    let mut sync_attempts = 0;
    let update = store
        .update_with_replace(Some("command-exact:new"), |path, bytes| {
            atomic_replace_with_sync(
                path,
                |file| file.write_all(bytes),
                || {
                    sync_attempts += 1;
                    if sync_attempts == 1 {
                        Ok(())
                    } else {
                        Err(io::Error::new(
                            io::ErrorKind::StorageFull,
                            "synthetic post-commit sync failure",
                        ))
                    }
                },
            )
        })
        .unwrap();
    // Use the same publication path as persist_always_allowed, under the lock.
    // It is a successful receipt, not a failed approval after the disk commit.
    let receipt = update.publish(&registry.always_allowed);
    drop(store);
    assert_eq!(sync_attempts, 2);
    assert!(matches!(
        &receipt,
        PersistenceReceipt::DurabilityUnconfirmed(error) if error.kind() == io::ErrorKind::StorageFull
    ));
    let warning = receipt
        .durability_warning()
        .expect("committed durability warning");
    assert_eq!(warning["level"], "warn");
    assert_eq!(warning["event"], "always_allow_durability_unconfirmed");
    assert_eq!(warning["committed"], true);
    assert_eq!(warning["active"], true);
    assert_eq!(warning["durability"], "unconfirmed");
    let message = warning["error"].as_str().unwrap();
    assert!(message.contains("was committed and is active"));
    assert!(message.contains("durability could not be confirmed"));
    assert!(!message.contains("not committed"));
    assert!(!warning.to_string().contains("command-exact:new"));
    receipt.warn_if_unconfirmed();
    let expected = HashSet::from([
        "command-exact:old".to_owned(),
        "command-exact:new".to_owned(),
    ]);
    assert_eq!(fixture.rules(), expected);
    assert_eq!(*registry.always_allowed.lock().unwrap(), expected);
    assert_eq!(*fixture.registry().always_allowed.lock().unwrap(), expected);
    assert!(fixture.temporaries().is_empty());
}

#[test]
fn post_commit_pruning_sync_failure_loads_the_rules_that_are_already_active() {
    let fixture = Fixture::new();
    let sensitive = "command-exact:API_KEY=synthetic-fixture-value probe";
    assert!(rule_carries_credentials(sensitive));
    fixture.seed(&[sensitive, "command-exact:clean"]);
    let store = LockedStore::acquire(&fixture.store).unwrap();
    let mut sync_attempts = 0;
    let update = store
        .update_with_replace(None, |path, bytes| {
            atomic_replace_with_sync(
                path,
                |file| file.write_all(bytes),
                || {
                    sync_attempts += 1;
                    if sync_attempts == 1 {
                        Ok(())
                    } else {
                        Err(io::Error::new(
                            io::ErrorKind::StorageFull,
                            "synthetic post-pruning sync failure",
                        ))
                    }
                },
            )
        })
        .unwrap();
    assert!(matches!(
        &update.commit,
        PersistenceReceipt::DurabilityUnconfirmed(_)
    ));
    // Startup must report the warning and return the successfully pruned state.
    let loaded = update.into_rules();
    drop(store);
    assert_eq!(sync_attempts, 2);
    let expected = HashSet::from(["command-exact:clean".to_owned()]);
    assert_eq!(loaded, expected);
    assert_eq!(fixture.rules(), expected);
    assert_eq!(*fixture.registry().always_allowed.lock().unwrap(), expected);
    assert!(fixture.temporaries().is_empty());
}

#[test]
fn stale_memory_cannot_resurrect_rules_removed_by_a_cooperating_writer() {
    let fixture = Fixture::new();
    fixture.seed(&["command-exact:removed"]);
    let registry = fixture.registry();
    fixture.seed(&["command-exact:peer-added"]);
    persist_in_runtime(&registry, "command-exact:approved-now").unwrap();
    let expected = HashSet::from([
        "command-exact:peer-added".to_owned(),
        "command-exact:approved-now".to_owned(),
    ]);
    assert_eq!(fixture.rules(), expected);
    assert_eq!(*registry.always_allowed.lock().unwrap(), expected);
}

#[test]
fn loading_and_pruning_waits_for_the_writer_and_keeps_its_addition() {
    let fixture = Fixture::new();
    let sensitive = "command-exact:API_KEY=synthetic-fixture-value probe";
    assert!(rule_carries_credentials(sensitive));
    fixture.seed(&[sensitive, "command-exact:clean"]);
    let store = LockedStore::acquire(&fixture.store).unwrap();
    let path = fixture.store.clone();
    let (started, starting) = mpsc::channel();
    let (finished, completion) = mpsc::channel();
    let loader = std::thread::spawn(move || {
        started.send(()).unwrap();
        let result = ToolRegistry::load_always_allowed(&path);
        finished.send(()).unwrap();
        result
    });
    let started_loading = starting.recv_timeout(WAIT);
    let completed_early = completion.recv_timeout(Duration::from_millis(100));
    // This writer commits before the loader can acquire the same stable lock.
    let _rules = store
        .update(Some("command-exact:writer-added"))
        .unwrap()
        .into_rules();
    drop(store);
    let loaded = loader.join().unwrap().unwrap();
    assert!(started_loading.is_ok());
    assert!(matches!(
        completed_early,
        Err(mpsc::RecvTimeoutError::Timeout)
    ));
    let expected = HashSet::from([
        "command-exact:clean".to_owned(),
        "command-exact:writer-added".to_owned(),
    ]);
    assert_eq!(loaded, expected);
    assert_eq!(fixture.rules(), expected);
    assert!(
        !std::fs::read_to_string(&fixture.store)
            .unwrap()
            .contains(sensitive)
    );
}

#[test]
fn a_new_commit_prunes_sensitive_rules_added_after_registry_load() {
    let fixture = Fixture::new();
    let registry = fixture.registry();
    fixture.seed(&[
        "command-exact:API_KEY=synthetic-fixture-value probe",
        "command-exact:peer-clean",
    ]);
    persist_in_runtime(&registry, "command-exact:approved").unwrap();
    assert_eq!(
        fixture.rules(),
        HashSet::from([
            "command-exact:peer-clean".to_owned(),
            "command-exact:approved".to_owned(),
        ])
    );
}

#[test]
fn malformed_disk_state_fails_without_replacing_or_publishing() {
    let fixture = Fixture::new();
    let registry = fixture.registry();
    let store = LockedStore::acquire(&fixture.store).unwrap();
    let malformed = b"[\"command-exact:old\", incomplete";
    assert_durable(atomic_replace(&fixture.store, |file| file.write_all(malformed)).unwrap());
    drop(store);
    let error = persist_in_runtime(&registry, "command-exact:new".to_owned());
    assert!(matches!(error, Err(ToolError::Network(_))));
    assert_eq!(std::fs::read(&fixture.store).unwrap(), malformed);
    assert!(registry.always_allowed.lock().unwrap().is_empty());
    assert!(fixture.temporaries().is_empty());
}

#[cfg(unix)]
#[test]
fn commit_rechecks_store_permissions_without_changing_the_old_file() {
    use std::os::unix::fs::PermissionsExt;
    let fixture = Fixture::new();
    fixture.seed(&["command-exact:old"]);
    let registry = fixture.registry();
    let original = std::fs::read(&fixture.store).unwrap();
    std::fs::set_permissions(&fixture.store, std::fs::Permissions::from_mode(0o644)).unwrap();
    let result = persist_in_runtime(&registry, "command-exact:new".to_owned());
    assert!(
        matches!(result, Err(ToolError::Io(ref error)) if error.kind() == io::ErrorKind::PermissionDenied)
    );
    assert_eq!(std::fs::read(&fixture.store).unwrap(), original);
    assert_eq!(
        *registry.always_allowed.lock().unwrap(),
        HashSet::from(["command-exact:old".to_owned()])
    );
}

#[cfg(unix)]
#[test]
fn temporary_store_and_sidecar_are_private_and_lock_inode_is_stable() {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    let fixture = Fixture::new();
    let registry = fixture.registry();
    let lock = fixture.store.with_extension("json.lock");
    let inode = std::fs::metadata(&lock).unwrap().ino();
    for signature in ["command-exact:first", "command-exact:second"] {
        persist_in_runtime(&registry, signature.to_owned()).unwrap();
        for path in [&fixture.store, &lock] {
            assert_eq!(
                std::fs::metadata(path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        assert_eq!(std::fs::metadata(&lock).unwrap().ino(), inode);
    }
    let guard = LockedStore::acquire(&fixture.store).unwrap();
    assert_durable(
        atomic_replace(&fixture.store, |file| {
            assert_eq!(file.metadata()?.permissions().mode() & 0o777, 0o600);
            file.write_all(b"[]")
        })
        .unwrap(),
    );
    drop(guard);
    assert!(fixture.temporaries().is_empty());
}

#[test]
fn sensitive_explicit_additions_never_publish_in_memory_or_on_disk() {
    let sensitive = concat!("command-exact:API_", "KEY=", "synthetic-fixture-value");
    assert!(rule_carries_credentials(sensitive));
    for persistent in [false, true] {
        let fixture = Fixture::new();
        let registry = ToolRegistry::new(&fixture.root, ApprovalMode::Strict).unwrap();
        let registry = if persistent {
            registry
                .with_always_allow_store(fixture.store.clone())
                .unwrap()
        } else {
            registry
        };
        assert!(matches!(
            persist_in_runtime(&registry, sensitive.to_owned()),
            Err(ToolError::ApprovalDenied(_))
        ));
        assert!(registry.always_allowed.lock().unwrap().is_empty());
        assert!(!fixture.store.exists());
    }
}

#[test]
fn invalid_store_errors_do_not_echo_rejected_values() {
    let fixture = Fixture::new();
    let rejected = "synthetic-private-store-value";
    let bytes = serde_json::to_vec(rejected).unwrap();
    {
        let _store = LockedStore::acquire(&fixture.store).unwrap();
        assert_durable(atomic_replace(&fixture.store, |file| file.write_all(&bytes)).unwrap());
    }
    let error = ToolRegistry::load_always_allowed(&fixture.store).unwrap_err();
    let message = error.to_string();
    assert!(matches!(error, ToolError::Network(_)));
    assert!(message.contains("line"));
    assert!(!message.contains(rejected));
    assert_eq!(std::fs::read(&fixture.store).unwrap(), bytes);
}

#[test]
fn a_filename_without_a_parent_uses_the_current_directory() {
    assert_eq!(store_parent(Path::new("always-allow.json")), Path::new("."));
}
