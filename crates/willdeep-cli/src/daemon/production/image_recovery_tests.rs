use super::*;

fn image_unknown(count: usize) -> (Directory, ProductionWorkflow, Reservation, String) {
    let (root, mut workflow) = fixture(1);
    image_retries::activate(
        &mut workflow,
        &Store::new(&root.0),
        &json!({"mode":"per_material_v1","legacyBindings":[]}),
        Uuid::new_v4(),
        1,
    )
    .unwrap();
    let subject = "d".repeat(64);
    let mut last = None;
    for index in 0..count {
        let reservation = request(&mut workflow, GenerationKind::Image);
        image_retries::bind_reservation(&mut workflow, &reservation, Some(&subject)).unwrap();
        budget::reserve(&mut workflow, &reservation).unwrap();
        if index + 1 == count {
            budget::unknown(&mut workflow, reservation.request_id).unwrap();
        } else {
            budget::settle(
                &mut workflow,
                reservation.request_id,
                reservation.quantities.clone(),
                true,
                "e".repeat(64),
            )
            .unwrap();
        }
        last = Some(reservation);
    }
    let original = last.unwrap();
    let position = workflow
        .nodes
        .iter()
        .position(|node| node.id == original.node_id)
        .unwrap();
    for node in &mut workflow.nodes[..position] {
        node.state = NodeState::Completed;
    }
    workflow.nodes[position].attempt = 5;
    workflow.status = ProductionStatus::WaitingUser;
    save(&root.0, &workflow);
    (root, workflow, original, subject)
}

fn save(root: &Path, workflow: &ProductionWorkflow) {
    Store::new(root)
        .update(workflow.id, |saved| {
            *saved = workflow.clone();
            Ok(())
        })
        .unwrap();
}

async fn authorized_resume(
    root: &Path,
    workflow: &ProductionWorkflow,
    value: Value,
) -> Result<Value> {
    operation(
        root,
        Uuid::new_v4(),
        "production.resume",
        json!({
            "id":workflow.id,"detail":false,"retryUnknownImageRequests":value
        }),
    )
    .await
}

fn authorization(request: &Reservation, subject: &str) -> Value {
    json!([{"requestID":request.request_id,"subject":subject}])
}

#[tokio::test]
async fn image_recovery_requires_an_explicit_exact_receipt_authorization() {
    let (root, workflow, _, _) = image_unknown(1);
    let result = operation(
        &root.0,
        Uuid::new_v4(),
        "production.resume",
        json!({"id":workflow.id}),
    )
    .await;
    assert!(result.is_err());
    assert_eq!(
        Store::new(&root.0).get(workflow.id).unwrap().generations,
        workflow.generations
    );
}

#[tokio::test]
async fn image_recovery_retains_usage_and_authorizes_only_one_new_purchase() {
    let (root, workflow, original, subject) = image_unknown(1);
    let result = authorized_resume(&root.0, &workflow, authorization(&original, &subject)).await;
    assert!(
        result.is_ok(),
        "exact human-authorized image recovery was rejected: {result:?}"
    );
    let mut current = Store::new(&root.0).get(workflow.id).unwrap();
    assert_eq!(current.generations, workflow.generations);
    assert_eq!(current.input, workflow.input);
    assert_eq!(
        budget::usage(&current).unwrap().reserved[&ProductionUnit::Images],
        1
    );
    assert_eq!(
        budget::usage(&current).unwrap().unknown_requests,
        vec![original.request_id]
    );
    assert!(budget::can_reconcile(&current).unwrap());
    let replacement = request(&mut current, GenerationKind::Image);
    image_retries::bind_reservation(&mut current, &replacement, Some(&subject)).unwrap();
    assert_eq!(
        budget::reserve(&mut current, &replacement).unwrap(),
        Reserved::Fresh
    );
    image_retries::bind_reservation(&mut current, &replacement, Some(&subject)).unwrap();
    assert!(matches!(
        budget::reserve(&mut current, &replacement).unwrap(),
        Reserved::Existing(_)
    ));
    assert_eq!(current.generations.len(), 2);
    assert_eq!(current.generations[0], workflow.generations[0]);
    assert_eq!(
        image_retries::status(&current, &subject).unwrap()["attempts"],
        2
    );
    budget::settle(
        &mut current,
        replacement.request_id,
        replacement.quantities.clone(),
        true,
        "e".repeat(64),
    )
    .unwrap();
    let third = request(&mut current, GenerationKind::Image);
    assert!(image_retries::bind_reservation(&mut current, &third, Some(&subject)).is_err());
    assert_eq!(
        budget::usage(&current).unwrap().reserved[&ProductionUnit::Images],
        1
    );
    assert_eq!(
        budget::usage(&current).unwrap().used[&ProductionUnit::Images],
        1
    );
    assert_eq!(
        budget::usage(&current).unwrap().unknown_requests,
        vec![original.request_id]
    );
}

#[tokio::test]
async fn replacement_image_unknown_does_not_inherit_the_original_permission() {
    let (root, workflow, original, subject) = image_unknown(1);
    assert!(
        authorized_resume(&root.0, &workflow, authorization(&original, &subject))
            .await
            .is_ok()
    );
    let mut current = Store::new(&root.0).get(workflow.id).unwrap();
    let replacement = request(&mut current, GenerationKind::Image);
    image_retries::bind_reservation(&mut current, &replacement, Some(&subject)).unwrap();
    budget::reserve(&mut current, &replacement).unwrap();
    budget::unknown(&mut current, replacement.request_id).unwrap();
    assert!(!budget::can_reconcile(&current).unwrap());
    assert_eq!(
        image_retries::status(&current, &subject).unwrap()["unresolved"],
        true
    );
    let third = request(&mut current, GenerationKind::Image);
    assert!(image_retries::bind_reservation(&mut current, &third, Some(&subject)).is_err());
    current.status = ProductionStatus::WaitingUser;
    save(&root.0, &current);
    assert!(
        authorized_resume(&root.0, &current, authorization(&original, &subject))
            .await
            .is_err()
    );
}

#[tokio::test]
async fn image_recovery_rejects_wrong_identity_kind_stage_and_active_requests() {
    for case in [
        "wrong_id",
        "wrong_subject",
        "extra_field",
        "cancelled",
        "running",
        "upstream",
        "other_stage",
        "other_kind",
        "inflight",
        "no_policy",
        "duplicate",
    ] {
        let (root, mut workflow, original, subject) = image_unknown(1);
        let mut value = authorization(&original, &subject);
        match case {
            "wrong_id" => value[0]["requestID"] = json!(Uuid::new_v4()),
            "wrong_subject" => value[0]["subject"] = json!("b".repeat(64)),
            "extra_field" => value[0]["override"] = json!(true),
            "cancelled" => workflow.cancel_requested = true,
            "running" => workflow.status = ProductionStatus::Running,
            "upstream" => workflow.generations[0].external_id = Some("existing-job".into()),
            "other_stage" => {
                workflow
                    .nodes
                    .iter_mut()
                    .find(|node| node.id == original.node_id)
                    .unwrap()
                    .stage = ProductionStage::GenerateVideo
            }
            "other_kind" => workflow.generations[0].kind = GenerationKind::Video,
            "inflight" => {
                let mut inflight = workflow.generations[0].clone();
                inflight.request_id = Uuid::new_v4();
                inflight.state = GenerationState::Reserved;
                workflow.generations.push(inflight);
            }
            "no_policy" => workflow.control_history.clear(),
            "duplicate" => value = json!([value[0], value[0]]),
            _ => unreachable!(),
        }
        save(&root.0, &workflow);
        assert!(
            authorized_resume(&root.0, &workflow, value).await.is_err(),
            "case {case}"
        );
        assert_eq!(
            Store::new(&root.0).get(workflow.id).unwrap().generations,
            workflow.generations,
            "case {case}"
        );
    }
}

#[tokio::test]
async fn image_recovery_cannot_reset_the_four_request_material_limit() {
    let (root, workflow, original, subject) = image_unknown(4);
    assert!(
        authorized_resume(&root.0, &workflow, authorization(&original, &subject))
            .await
            .is_err()
    );
    assert_eq!(
        Store::new(&root.0).get(workflow.id).unwrap().generations,
        workflow.generations
    );
}

#[tokio::test]
async fn image_recovery_permission_is_bound_to_the_unchanged_original_receipt() {
    let (root, workflow, original, subject) = image_unknown(1);
    assert!(
        authorized_resume(&root.0, &workflow, authorization(&original, &subject))
            .await
            .is_ok()
    );
    let mut current = Store::new(&root.0).get(workflow.id).unwrap();
    current.generations[0].request_digest = "c".repeat(64);
    assert!(!budget::can_reconcile(&current).unwrap());
    assert_eq!(
        image_retries::status(&current, &subject).unwrap()["unresolved"],
        true
    );
}
