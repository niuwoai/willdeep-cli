use super::*;

#[tokio::test]
async fn queued_resolution_checks_customer_intent_before_loading_any_agent() {
    let home = home();
    let mut session = CustomerSession {
        origin: "https://some.invalid".into(),
        token: "fixture-only".into(),
        user_id: "account-a".into(),
        tenant_id: "tenant-a".into(),
    };
    let request = json!({"id":"user/not-installed","choice":"keep_local","expected_local_revision":"a".repeat(64),"expected_remote_etag":"etag-a","expected_account":{"origin":session.origin,"user_id":session.user_id,"tenant_id":session.tenant_id}});
    session.user_id = "account-b".into();
    session.save(&home).unwrap();
    let error = crate::agent_registry::operate(&home, "resolve", request)
        .await
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("account changed before queued resolution")
    );
    assert!(!session.library(&home).root().exists());
    std::fs::remove_dir_all(home).unwrap();
}

struct Pair {
    home: PathBuf,
    other: PathBuf,
    session: CustomerSession,
    db: DatabaseState,
    task: tokio::task::JoinHandle<()>,
    local: AgentPackage,
    cloud: AgentPackage,
    etag: String,
}
impl Drop for Pair {
    fn drop(&mut self) {
        self.task.abort();
        let _ = std::fs::remove_dir_all(&self.home);
        let _ = std::fs::remove_dir_all(&self.other);
    }
}
fn edited(original: &AgentPackage, version: &str, text: &str) -> AgentPackage {
    let mut definition = original.definition.clone();
    definition.version = version.into();
    definition.instructions.push_str(text);
    AgentPackage::local(definition).unwrap()
}
async fn pair() -> Pair {
    let (origin, db, task) = server().await;
    let session = Service::new(&origin)
        .unwrap()
        .identify("customer-test-session")
        .await
        .unwrap();
    let a = home();
    let b = home();
    session.save(&a).unwrap();
    session.save(&b).unwrap();
    let original = package();
    session
        .library(&a)
        .install(&original, Origin::Private, &BTreeMap::new())
        .unwrap();
    synchronize(&a).await.unwrap();
    synchronize(&b).await.unwrap();
    let cloud = edited(&original, "1.0.0-rc2", " Device A");
    let local = edited(&original, "1.0.0-rc2", " Device B");
    for (home, package) in [(&a, &cloud), (&b, &local)] {
        let library = session.library(home);
        let etag = library.installations().unwrap()[&original.definition.definition_id]
            .remote_etag
            .clone()
            .unwrap();
        library
            .install(package, Origin::Private, &BTreeMap::new())
            .unwrap();
        library
            .set_remote_base(&package.definition.definition_id, etag)
            .unwrap();
    }
    synchronize(&a).await.unwrap();
    let report = synchronize(&b).await.unwrap();
    assert_eq!(report.conflict_details.len(), 1);
    assert_eq!(
        report.conflict_details[0].expected_local_revision,
        local.sha256
    );
    let etag = report.conflict_details[0].expected_remote_etag.clone();
    Pair {
        home: b,
        other: a,
        session,
        db,
        task,
        local,
        cloud,
        etag,
    }
}

#[tokio::test]
async fn explicit_keep_local_converges_both_devices_and_preserves_both_branches() {
    let p = pair().await;
    let id = &p.local.definition.definition_id;
    assert!(
        resolve(
            &p.home,
            id,
            ResolveChoice::KeepLocal,
            &p.local.sha256,
            &p.etag,
            None
        )
        .await
        .unwrap_err()
        .to_string()
        .contains("--new-version")
    );
    let result = resolve(
        &p.home,
        id,
        ResolveChoice::KeepLocal,
        &p.local.sha256,
        &p.etag,
        Some("1.0.0-rc3"),
    )
    .await
    .unwrap();
    assert_eq!(result["version"], "1.0.0-rc3");
    let mut expected = p.local.definition.clone();
    expected.version = "1.0.0-rc3".into();
    let actual = p
        .session
        .library(&p.home)
        .package(result["revision"].as_str().unwrap())
        .unwrap();
    assert_eq!(actual.definition, expected);
    assert!(!p.session.library(&p.home).installations().unwrap()[id].dirty);
    let report = synchronize(&p.other).await.unwrap();
    // An explicit cloud choice is also required for same-version divergent branches.
    if !report.conflicts.is_empty() {
        let conflict = &report.conflict_details[0];
        resolve(
            &p.other,
            id,
            ResolveChoice::UseCloud,
            &p.cloud.sha256,
            &conflict.expected_remote_etag,
            None,
        )
        .await
        .unwrap();
    }
    assert_eq!(
        p.session.library(&p.other).installations().unwrap()[id].sha256,
        result["revision"].as_str().unwrap()
    );
    for package in [&p.local, &p.cloud] {
        assert_eq!(
            p.session.library(&p.home).package(&package.sha256).unwrap(),
            *package
        );
    }
}

#[tokio::test]
async fn final_store_compare_and_swap_rejects_new_local_revision_for_cloud_and_deletion() {
    let p = pair().await;
    let id = &p.local.definition.definition_id;
    let library = p.session.library(&p.home);
    let newer = edited(&p.local, "1.0.0-rc3", " New local edit");
    library
        .install(&newer, Origin::Private, &BTreeMap::new())
        .unwrap();
    for package in [None, Some(&p.cloud)] {
        assert!(
            library
                .resolve_private(id, &p.local.sha256, package, &p.etag, true)
                .is_err()
        );
        assert_eq!(library.installations().unwrap()[id].sha256, newer.sha256);
    }
}

#[tokio::test]
async fn explicit_cloud_choice_replaces_a_pin_but_retains_local_binding_and_history() {
    let p = pair().await;
    let id = &p.local.definition.definition_id;
    let library = p.session.library(&p.home);
    library.set_pinned(id, true).unwrap();
    library
        .set_binding(
            id,
            willdeep_core::agent_definition::store::ModelBinding {
                provider_profile: Some("local-only".into()),
                model: None,
            },
        )
        .unwrap();
    assert_eq!(
        synchronize(&p.home).await.unwrap().conflicts,
        vec![id.clone()]
    );
    resolve(
        &p.home,
        id,
        ResolveChoice::UseCloud,
        &p.local.sha256,
        &p.etag,
        None,
    )
    .await
    .unwrap();
    let item = library.installations().unwrap().remove(id).unwrap();
    assert_eq!(item.sha256, p.cloud.sha256);
    assert!(item.pinned);
    assert!(!item.dirty);
    assert_eq!(item.binding.provider_profile.as_deref(), Some("local-only"));
    assert!(library.package(&p.local.sha256).is_ok());
    assert!(synchronize(&p.home).await.unwrap().conflicts.is_empty());
}

#[tokio::test]
async fn tombstone_requires_explicit_restore_or_accept_and_keeps_history() {
    for choice in [ResolveChoice::KeepLocal, ResolveChoice::UseCloud] {
        let p = pair().await;
        let id = &p.local.definition.definition_id;
        Service::new(&p.session.origin)
            .unwrap()
            .delete(&p.session, id, &p.etag, "delete-cloud")
            .await
            .unwrap();
        let report = synchronize(&p.home).await.unwrap();
        let conflict = &report.conflict_details[0];
        assert!(conflict.remote_deleted);
        resolve(
            &p.home,
            id,
            choice,
            &p.local.sha256,
            &conflict.expected_remote_etag,
            matches!(choice, ResolveChoice::KeepLocal).then_some("1.0.0-rc3"),
        )
        .await
        .unwrap();
        let items = p.session.library(&p.home).installations().unwrap();
        match choice {
            ResolveChoice::KeepLocal => {
                let resolved = p
                    .session
                    .library(&p.home)
                    .package(&items[id].sha256)
                    .unwrap();
                assert_eq!(
                    resolved.definition.instructions,
                    p.local.definition.instructions
                );
                assert_eq!(resolved.definition.version, "1.0.0-rc3");
                assert!(!p.db.lock().await.item.as_ref().unwrap().deleted);
            }
            ResolveChoice::UseCloud => {
                assert!(!items.contains_key(id));
                assert!(p.db.lock().await.item.as_ref().unwrap().deleted);
            }
        }
        assert!(p.session.library(&p.home).package(&p.local.sha256).is_ok());
    }
}

#[tokio::test]
async fn stale_cloud_etag_and_server_side_race_never_overwrite_new_cloud_revision() {
    for race_at_put in [false, true] {
        let p = pair().await;
        let id = &p.local.definition.definition_id;
        let newest = edited(&p.cloud, "1.0.0-rc3", " New remote edit");
        let mut remote = p.db.lock().await.item.clone().unwrap();
        remote.etag = "newer-cloud".into();
        remote.sha256 = newest.sha256.clone();
        remote.version = newest.definition.version.clone();
        remote.package = Some(newest.clone());
        if race_at_put {
            p.db.lock().await.before_put = Some(remote);
        } else {
            p.db.lock().await.item = Some(remote);
        }
        assert!(
            resolve(
                &p.home,
                id,
                ResolveChoice::KeepLocal,
                &p.local.sha256,
                &p.etag,
                Some("1.0.0-rc4"),
            )
            .await
            .is_err()
        );
        assert_eq!(
            p.db.lock().await.item.as_ref().unwrap().sha256,
            newest.sha256
        );
        assert!(p.session.library(&p.home).installations().unwrap()[id].dirty);
    }
}

#[tokio::test]
async fn local_edit_during_remote_read_cancels_both_choices_without_upload() {
    for choice in [ResolveChoice::KeepLocal, ResolveChoice::UseCloud] {
        let p = pair().await;
        let id = &p.local.definition.definition_id;
        let newer = edited(&p.local, "1.0.0-rc3", " New local edit");
        let library = p.session.library(&p.home);
        let changed = newer.clone();
        p.db.lock().await.after_list = Some(Box::new(move || {
            library
                .install(&changed, Origin::Private, &BTreeMap::new())
                .unwrap()
        }));
        assert!(
            resolve(
                &p.home,
                id,
                choice,
                &p.local.sha256,
                &p.etag,
                Some("1.0.0-rc4")
            )
            .await
            .is_err()
        );
        assert_eq!(
            p.session.library(&p.home).installations().unwrap()[id].sha256,
            newer.sha256
        );
        assert_eq!(p.db.lock().await.put_keys.len(), 2);
    }
}

#[tokio::test]
async fn switching_account_during_resolution_cancels_before_any_write() {
    for choice in [ResolveChoice::KeepLocal, ResolveChoice::UseCloud] {
        let p = pair().await;
        let id = &p.local.definition.definition_id;
        let mut switched = p.session.clone();
        switched.user_id = "other-user".into();
        let home = p.home.clone();
        p.db.lock().await.after_list = Some(Box::new(move || switched.save(&home).unwrap()));
        assert!(
            resolve(
                &p.home,
                id,
                choice,
                &p.local.sha256,
                &p.etag,
                Some("1.0.0-rc4")
            )
            .await
            .unwrap_err()
            .to_string()
            .contains("account changed")
        );
        assert_eq!(p.db.lock().await.put_keys.len(), 2);
        assert_eq!(
            p.session.library(&p.home).installations().unwrap()[id].sha256,
            p.local.sha256
        );
    }
}
