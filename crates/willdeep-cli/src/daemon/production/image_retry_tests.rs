use super::*;

fn enable_image_policy(root: &Path, workflow: &mut ProductionWorkflow) {
    image_retries::activate(
        workflow,
        &Store::new(root),
        &json!({"mode":"per_material_v1","legacyBindings":[]}),
        Uuid::new_v4(),
        1,
    )
    .unwrap();
}

#[test]
fn material_limit_counts_failed_requests_and_replay_without_reset() {
    let (root, mut workflow) = fixture(1);
    enable_image_policy(&root.0, &mut workflow);
    let subject = "a".repeat(64);
    let mut last = None;
    for _ in 0..4 {
        let reservation = request(&mut workflow, GenerationKind::Image);
        image_retries::bind_reservation(&mut workflow, &reservation, Some(&subject)).unwrap();
        budget::reserve(&mut workflow, &reservation).unwrap();
        budget::settle(
            &mut workflow,
            reservation.request_id,
            [(ProductionUnit::Images, 0)].into(),
            false,
            "e".repeat(64),
        )
        .unwrap();
        last = Some(reservation);
    }
    let last = last.unwrap();
    let frozen = workflow.clone();
    image_retries::bind_reservation(&mut workflow, &last, Some(&subject)).unwrap();
    assert!(matches!(
        budget::reserve(&mut workflow, &last).unwrap(),
        Reserved::Existing(_)
    ));
    assert_eq!(workflow, frozen);
    let fifth = request(&mut workflow, GenerationKind::Image);
    assert!(
        image_retries::bind_reservation(&mut workflow, &fifth, Some(&subject))
            .unwrap_err()
            .to_string()
            .contains("material_limit")
    );
    assert_eq!(workflow.generations.len(), 4);
    assert_eq!(
        image_retries::status(&workflow, &subject).unwrap()["attempts"],
        4
    );
}

#[test]
fn unknown_images_are_counted_and_never_grant_another_purchase() {
    let (root, mut workflow) = fixture(1);
    enable_image_policy(&root.0, &mut workflow);
    let subject = "b".repeat(64);
    let first = request(&mut workflow, GenerationKind::Image);
    image_retries::bind_reservation(&mut workflow, &first, Some(&subject)).unwrap();
    budget::reserve(&mut workflow, &first).unwrap();
    budget::unknown(&mut workflow, first.request_id).unwrap();
    let second = request(&mut workflow, GenerationKind::Image);
    assert!(image_retries::bind_reservation(&mut workflow, &second, Some(&subject)).is_err());
    assert_eq!(
        image_retries::status(&workflow, &subject).unwrap()["unresolved"],
        true
    );
    assert_eq!(workflow.generations.len(), 1);
}

#[test]
fn legacy_binding_requires_complete_verified_responses_and_cannot_be_reset() {
    let (root, mut workflow) = fixture(1);
    let reservation = request(&mut workflow, GenerationKind::Image);
    budget::reserve(&mut workflow, &reservation).unwrap();
    let saved = json!({"response":{"filePath":"/private/tmp/legacy.png"},"actual":{"images":1},"succeeded":true,"requestDigest":reservation.request_digest});
    let store = Store::new(&root.0);
    store
        .save_receipt(workflow.id, reservation.request_id, &saved)
        .unwrap();
    budget::settle(
        &mut workflow,
        reservation.request_id,
        reservation.quantities.clone(),
        true,
        engine::digest(&saved).unwrap(),
    )
    .unwrap();
    let receipts = workflow.generations.clone();
    let mut policy = json!({"mode":"per_material_v1","legacyBindings":[{"requestID":reservation.request_id,"subject":"c".repeat(64),"fileName":"wrong.png"}]});
    assert!(image_retries::activate(&mut workflow, &store, &policy, Uuid::new_v4(), 1).is_err());
    policy["legacyBindings"][0]["fileName"] = json!("legacy.png");
    image_retries::activate(&mut workflow, &store, &policy, Uuid::new_v4(), 1).unwrap();
    assert_eq!(
        image_retries::status(&workflow, &"c".repeat(64)).unwrap()["attempts"],
        1
    );
    assert_eq!(workflow.generations, receipts);
    policy["legacyBindings"] = json!([]);
    assert!(image_retries::activate(&mut workflow, &store, &policy, Uuid::new_v4(), 2).is_err());
    assert_eq!(workflow.generations, receipts);
}

#[test]
fn unmapped_old_images_and_missing_subjects_fail_closed() {
    let (root, mut workflow) = fixture(1);
    enable_image_policy(&root.0, &mut workflow);
    let first = request(&mut workflow, GenerationKind::Image);
    assert!(image_retries::bind_reservation(&mut workflow, &first, None).is_err());
    budget::reserve(&mut workflow, &first).unwrap();
    assert!(image_retries::status(&workflow, &"d".repeat(64)).is_err());
}

#[test]
fn authorized_image_dispatch_continues_old_batch_numbers_without_extending_other_media() {
    let (root, mut workflow) = fixture(1);
    enable_image_policy(&root.0, &mut workflow);
    for node in &mut workflow.nodes {
        node.state = NodeState::Completed;
    }
    let node = workflow
        .nodes
        .iter_mut()
        .find(|node| node.stage == ProductionStage::GenerateAssets)
        .unwrap();
    node.state = NodeState::Pending;
    node.attempt = 4;
    let id = workflow.id;
    Store::new(&root.0)
        .update(id, |saved| {
            *saved = workflow.clone();
            Ok(())
        })
        .unwrap();
    let prepared = engine::prepare(&root.0, id).unwrap();
    assert_eq!(prepared.attempt, 5);
    assert_eq!(
        prepared.arguments["input"]["imageRetryPolicy"],
        "per_material_v1"
    );
    for stage in [
        ProductionStage::GenerateVideo,
        ProductionStage::GenerateDialogue,
        ProductionStage::GenerateMusic,
    ] {
        assert_eq!(image_retries::node_limit(&workflow, stage), 4);
    }
    assert_eq!(
        Store::new(&root.0).get(id).unwrap().input.limits,
        workflow.input.limits
    );
}
