use super::*;

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        Self(std::env::temp_dir().join(format!("willdeep-child-store-latency-{}", Uuid::new_v4())))
    }

    fn store(&self) -> ChildStore {
        ChildStore::new(&self.0)
    }

    fn bytes(&self) -> Vec<u8> {
        std::fs::read(self.0.join("runtime/session-children.json")).unwrap()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn unchanged_transactions_and_host_reads_do_not_rewrite_the_ledger() {
    let fixture = Fixture::new();
    let store = fixture.store();
    let parent = Uuid::new_v4();
    store
        .transaction(|ledger| {
            ledger.operator_input(parent);
            Ok(())
        })
        .unwrap();
    let before = fixture.bytes();
    let path = fixture.0.join("runtime/session-children.json");
    let modified = std::fs::metadata(&path).unwrap().modified().unwrap();
    for _ in 0..4 {
        assert!(store.host_context(parent).unwrap().is_empty());
        store.transaction(|ledger| store.persist(ledger)).unwrap();
    }
    assert_eq!(
        fixture.bytes(),
        before,
        "no-op reads must preserve revision"
    );
    assert_eq!(
        std::fs::metadata(path).unwrap().modified().unwrap(),
        modified
    );
}

#[test]
fn real_mutations_remain_durable_at_each_explicit_commit_barrier() {
    let fixture = Fixture::new();
    let parent = Uuid::new_v4();
    let first = Uuid::new_v4();
    let second = Uuid::new_v4();
    let store = fixture.store();
    store
        .transaction(|ledger| {
            assert!(ledger.reserve_slot(first, parent));
            store.persist(ledger)?;
            let disk: Ledger = serde_json::from_slice(&fixture.bytes()).unwrap();
            assert_eq!(disk.slots.get(&first), Some(&parent));
            assert!(!disk.slots.contains_key(&second));
            assert!(ledger.reserve_slot(second, parent));
            Ok(())
        })
        .unwrap();
    let bytes = fixture.bytes();
    let disk: Ledger = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(disk.slots.len(), 2);
    assert_eq!(disk.revision, 2, "each changed barrier commits once");
    fixture
        .store()
        .transaction(|ledger| {
            assert_eq!(ledger.slots, disk.slots);
            Ok(())
        })
        .unwrap();
    assert_eq!(fixture.bytes(), bytes, "reopening must not create a commit");
}

#[test]
fn failed_barrier_is_retried_even_when_the_renamed_bytes_match() {
    let fixture = Fixture::new();
    let store = fixture.store();
    let parent = Uuid::new_v4();
    let result = store.transaction(|ledger| {
        ledger.operator_input(parent);
        store.persist_with_barrier(ledger, || anyhow::bail!("injected directory sync failure"))
    });
    assert!(result.is_err());
    let bytes = fixture.bytes();
    let mut retried = false;
    let result = store.transaction(|ledger| {
        store.persist_with_barrier(ledger, || {
            retried = true;
            anyhow::bail!("injected barrier retry failure")
        })
    });
    assert!(result.is_err());
    assert!(
        retried,
        "matching bytes must not acknowledge a failed barrier"
    );
    store.transaction(|_| Ok(())).unwrap();
    assert_eq!(
        fixture.bytes(),
        bytes,
        "recovery does not invent a revision"
    );
    store
        .transaction(|ledger| {
            store.persist_with_barrier(ledger, || panic!("durable no-op must not sync again"))
        })
        .unwrap();
}

#[test]
fn reopened_store_establishes_durability_before_skipping_commits() {
    let fixture = Fixture::new();
    fixture.store().transaction(|_| Ok(())).unwrap();
    let before = fixture.bytes();
    let reopened = fixture.store();
    let mut checked = false;
    let result = reopened.transaction(|ledger| {
        reopened.persist_with_barrier(ledger, || {
            checked = true;
            anyhow::bail!("injected reopen sync failure")
        })
    });
    assert!(checked);
    assert!(result.is_err());
    reopened.transaction(|_| Ok(())).unwrap();
    assert_eq!(fixture.bytes(), before);
}

#[test]
fn failed_transactions_do_not_persist_uncommitted_changes() {
    let fixture = Fixture::new();
    let store = fixture.store();
    store.transaction(|_| Ok(())).unwrap();
    let before = fixture.bytes();
    let result: Result<()> = store.transaction(|ledger| {
        ledger.operator_input(Uuid::new_v4());
        anyhow::bail!("fixture failure before commit")
    });
    assert!(result.is_err());
    assert_eq!(fixture.bytes(), before);
}
