use super::*;
use budget::{Reservation, Reserved};
use std::collections::HashSet;

fn historical_hold(workflow: &ProductionWorkflow) -> historical::Hold {
    historical::Hold {
        tokens: 20,
        model_calls: 2,
        episode_calls: [(1, 2)].into_iter().collect(),
        evidence_digest: "e".repeat(64),
        generation_count: workflow.generations.len(),
        generations_digest: historical::receipts_digest(&workflow.generations).unwrap(),
    }
}

#[test]
fn historical_capacity_is_reserved_without_altering_unknown_receipts_or_limits() {
    let (_root, mut workflow) = fixture(20);
    let request = request(&mut workflow, GenerationKind::Model);
    budget::reserve(&mut workflow, &request).unwrap();
    workflow.generations[0].state = GenerationState::Unknown;
    let originals = workflow.generations.clone();
    let limits = workflow.input.limits.clone();
    let hold = historical_hold(&workflow);
    historical::apply(&mut workflow, hold.clone(), Uuid::new_v4(), 1).unwrap();
    let usage = budget::usage(&workflow).unwrap();
    assert_eq!(usage.historical_hold[&ProductionUnit::Tokens], 20);
    assert_eq!(usage.reserved[&ProductionUnit::Tokens], 30);
    assert_eq!(usage.reserved[&ProductionUnit::ModelCalls], 3);
    assert_eq!(usage.unknown_requests, vec![request.request_id]);
    assert!(usage.used.is_empty());
    assert_eq!(workflow.generations, originals);
    assert_eq!(workflow.input.limits, limits);
    assert!(workflow.budget_changes.is_empty());
    historical::apply(&mut workflow, hold.clone(), Uuid::new_v4(), 2).unwrap();
    assert_eq!(workflow.control_history.len(), 1);
    let mut reduced = hold;
    reduced.tokens -= 1;
    assert!(historical::apply(&mut workflow, reduced, Uuid::new_v4(), 3).is_err());
}

#[test]
fn historical_episode_calls_prevent_new_generation_at_the_original_episode_cap() {
    let (_root, mut workflow) = fixture(20);
    workflow.input.max_model_calls_per_episode = Some(3);
    let first = request(&mut workflow, GenerationKind::Model);
    budget::reserve(&mut workflow, &first).unwrap();
    budget::settle(
        &mut workflow,
        first.request_id,
        first.quantities.clone(),
        true,
        "e".repeat(64),
    )
    .unwrap();
    let hold = historical_hold(&workflow);
    historical::apply(&mut workflow, hold, Uuid::new_v4(), 1).unwrap();
    let next = request(&mut workflow, GenerationKind::Model);
    assert!(
        budget::reserve(&mut workflow, &next)
            .unwrap_err()
            .to_string()
            .contains("episode_1_model_calls")
    );
    assert_eq!(workflow.generations.len(), 1);
}

#[test]
fn historical_hold_rejects_excess_capacity_atomically_and_binds_original_reservations() {
    let (_root, mut workflow) = fixture(20);
    let first = request(&mut workflow, GenerationKind::Model);
    budget::reserve(&mut workflow, &first).unwrap();
    workflow
        .input
        .limits
        .insert(ProductionUnit::Tokens, Some(25));
    let original = workflow.clone();
    let hold = historical_hold(&workflow);
    assert!(historical::apply(&mut workflow, hold.clone(), Uuid::new_v4(), 1).is_err());
    assert_eq!(workflow, original);
    workflow
        .input
        .limits
        .insert(ProductionUnit::Tokens, Some(100));
    historical::apply(&mut workflow, hold, Uuid::new_v4(), 2).unwrap();
    workflow.generations[0]
        .reservation
        .insert(ProductionUnit::Tokens, 11);
    assert!(budget::usage(&workflow).is_err());
}

#[test]
fn historical_snapshot_survives_genuine_settlement_but_rejects_stale_installation() {
    let (_root, mut workflow) = fixture(20);
    let first = request(&mut workflow, GenerationKind::Model);
    budget::reserve(&mut workflow, &first).unwrap();
    let hold = historical_hold(&workflow);
    historical::apply(&mut workflow, hold.clone(), Uuid::new_v4(), 1).unwrap();
    budget::settle(
        &mut workflow,
        first.request_id,
        first.quantities.clone(),
        true,
        "e".repeat(64),
    )
    .unwrap();
    assert_eq!(
        budget::usage(&workflow).unwrap().reserved[&ProductionUnit::Tokens],
        20
    );
    let next = request(&mut workflow, GenerationKind::Model);
    budget::reserve(&mut workflow, &next).unwrap();
    workflow.control_history.clear();
    assert!(
        historical::apply(&mut workflow, hold, Uuid::new_v4(), 2)
            .unwrap_err()
            .to_string()
            .contains("snapshot_stale")
    );
}

struct Directory(PathBuf);
impl Drop for Directory {
    fn drop(&mut self) {
        if let Err(error) = std::fs::remove_dir_all(&self.0) {
            eprintln!("production fixture cleanup: {error}");
        }
    }
}

fn fixture(count: u16) -> (Directory, ProductionWorkflow) {
    let directory =
        Directory(std::env::temp_dir().join(format!("production-unit-{}", Uuid::new_v4())));
    std::fs::create_dir_all(&directory.0).unwrap();
    let limits = [
        (ProductionUnit::ModelCalls, 1_000),
        (ProductionUnit::Tokens, 5_000_000),
        (ProductionUnit::Images, 100),
        (ProductionUnit::TtsCharacters, 10_000),
        (ProductionUnit::MusicCalls, 10),
        (ProductionUnit::MusicSeconds, 900),
        (ProductionUnit::VideoCalls, 20),
        (ProductionUnit::VideoSeconds, 200),
    ]
    .into_iter()
    .map(|(unit, limit)| (unit, Some(limit)))
    .collect();
    let input = ProductionStart {
        brief: Some("县城旧照相馆，父女亲情".into()),
        source_screenplay_id: None,
        max_model_calls_per_episode: None,
        origin_session_id: None,
        workspace: directory.0.clone(),
        spec: SeriesSpec {
            episode_count: count,
            ..SeriesSpec::default()
        },
        limits,
    };
    let workflow = Store::new(&directory.0)
        .create(Uuid::new_v4(), input)
        .unwrap();
    (directory, workflow)
}

fn request(workflow: &mut ProductionWorkflow, kind: GenerationKind) -> Reservation {
    let stage = match kind {
        GenerationKind::Model => ProductionStage::WriteEpisode,
        GenerationKind::Image => ProductionStage::GenerateFrames,
        GenerationKind::Tts => ProductionStage::GenerateDialogue,
        GenerationKind::Music => ProductionStage::GenerateMusic,
        GenerationKind::Video => ProductionStage::GenerateVideo,
    };
    let node = workflow
        .nodes
        .iter_mut()
        .find(|node| node.stage == stage)
        .unwrap();
    node.state = NodeState::Running;
    node.grant = Uuid::new_v4();
    node.input_digest = "a".repeat(64);
    let quantities = kind
        .units()
        .iter()
        .map(|unit| {
            (
                *unit,
                if matches!(
                    unit,
                    ProductionUnit::ModelCalls
                        | ProductionUnit::Images
                        | ProductionUnit::MusicCalls
                        | ProductionUnit::VideoCalls
                ) {
                    1
                } else {
                    10
                },
            )
        })
        .collect();
    Reservation {
        request_id: Uuid::new_v4(),
        node_id: node.id,
        kind,
        request_digest: "b".repeat(64),
        quantities,
        provider: "test-provider".into(),
    }
}

#[test]
fn twenty_to_eighty_episodes_have_distinct_full_media_nodes_and_exact_final_coverage() {
    for count in [20, 40, 80] {
        let (_root, workflow) = fixture(count);
        let mut seen = HashSet::new();
        for node in &workflow.nodes {
            assert!(
                node.dependencies
                    .iter()
                    .all(|dependency| seen.contains(dependency))
            );
            assert!(seen.insert(node.id));
        }
        for stage in [
            ProductionStage::WriteEpisode,
            ProductionStage::GenerateFrames,
            ProductionStage::GenerateDialogue,
            ProductionStage::GenerateMusic,
            ProductionStage::GenerateVideo,
            ProductionStage::ReviewVideo,
            ProductionStage::ComposeEpisode,
            ProductionStage::ReviewFilm,
        ] {
            let actual: Vec<_> = workflow
                .nodes
                .iter()
                .filter(|node| node.stage == stage)
                .map(|node| node.episode.unwrap())
                .collect();
            assert_eq!(actual, (1..=count).collect::<Vec<_>>());
        }
        let coverage: Vec<_> = workflow
            .nodes
            .iter()
            .filter(|node| node.stage == ProductionStage::ReviewSeries)
            .filter_map(|node| node.range)
            .flat_map(|range| range.first..=range.last)
            .collect();
        assert_eq!(coverage, (1..=count).collect::<Vec<_>>());
        let delivery = workflow.nodes.last().unwrap();
        assert_eq!(delivery.stage, ProductionStage::Deliver);
        let global = workflow
            .nodes
            .iter()
            .find(|node| node.id == delivery.dependencies[0])
            .unwrap();
        assert_eq!(global.stage, ProductionStage::ReviewSeries);
        assert!(global.range.is_none());
        assert_eq!(
            global.dependencies.len(),
            workflow.input.spec.episode_ranges().len()
        );
    }
}

#[test]
fn same_start_id_preserves_nodes_and_generations_and_rejects_another_topic() {
    let (root, mut workflow) = fixture(80);
    let request = request(&mut workflow, GenerationKind::Image);
    Store::new(&root.0)
        .update(workflow.id, |saved| {
            *saved = workflow.clone();
            budget::reserve(saved, &request)?;
            Ok(())
        })
        .unwrap();
    let replay = Store::new(&root.0)
        .create(workflow.id, workflow.input.clone())
        .unwrap();
    assert_eq!(replay.nodes, workflow.nodes);
    assert_eq!(replay.generations.len(), 1);
    let mut conflict = workflow.input.clone();
    conflict.brief = Some("另一个题材".into());
    assert!(Store::new(&root.0).create(workflow.id, conflict).is_err());
}

#[test]
fn reservations_persist_before_dispatch_and_identical_retry_never_returns_fresh() {
    let (root, mut workflow) = fixture(20);
    let request = request(&mut workflow, GenerationKind::Video);
    let id = workflow.id;
    Store::new(&root.0)
        .update(id, |saved| {
            *saved = workflow;
            budget::reserve(saved, &request)
        })
        .unwrap();
    let mut recovered = Store::new(&root.0).get(id).unwrap();
    assert_eq!(recovered.generations[0].state, GenerationState::Reserved);
    assert!(matches!(
        budget::reserve(&mut recovered, &request).unwrap(),
        Reserved::Existing(_)
    ));
    assert_eq!(recovered.generations.len(), 1);
    assert_eq!(
        budget::usage(&recovered).unwrap().unknown_requests,
        vec![request.request_id]
    );
}

#[test]
fn changed_payload_quantity_provider_or_node_cannot_reuse_a_request_id() {
    let (_root, mut workflow) = fixture(20);
    let request = request(&mut workflow, GenerationKind::Video);
    budget::reserve(&mut workflow, &request).unwrap();
    let mut variants = vec![request.clone(); 4];
    variants[0].request_digest = "c".repeat(64);
    variants[1]
        .quantities
        .insert(ProductionUnit::VideoSeconds, 11);
    variants[2].provider = "another-provider".into();
    variants[3].node_id = Uuid::new_v4();
    for changed in variants {
        assert!(budget::reserve(&mut workflow, &changed).is_err());
    }
    assert_eq!(workflow.generations.len(), 1);
}

#[test]
fn explicitly_authorized_unknown_text_keeps_its_receipt_and_reservation_for_a_new_attempt() {
    let (root, mut workflow) = fixture(80);
    let first = request(&mut workflow, GenerationKind::Model);
    let index = workflow
        .nodes
        .iter()
        .position(|node| node.id == first.node_id)
        .unwrap();
    for node in &mut workflow.nodes[..index] {
        node.state = NodeState::Completed;
    }
    workflow.nodes[index].attempt = 5;
    budget::reserve(&mut workflow, &first).unwrap();
    budget::unknown(&mut workflow, first.request_id).unwrap();
    workflow.status = ProductionStatus::WaitingUser;
    let saved = workflow.generations[0].clone();
    assert!(!budget::can_reconcile(&workflow).unwrap());
    recovery::retry_unknown_text(&mut workflow, &[first.request_id], Uuid::new_v4(), 42).unwrap();
    assert_eq!(workflow.generations[0], saved);
    assert!(budget::can_reconcile(&workflow).unwrap());
    assert!(budget::usage(&workflow).unwrap().used.is_empty());
    assert_eq!(budget::usage(&workflow).unwrap().reserved, first.quantities);
    workflow.status = ProductionStatus::Running;
    Store::new(&root.0)
        .update(workflow.id, |current| {
            *current = workflow.clone();
            Ok(())
        })
        .unwrap();
    let next = engine::prepare(&root.0, workflow.id).unwrap();
    assert_eq!(next.id, first.node_id);
    assert_eq!(next.attempt, 6);
    assert_eq!(next.history[0]["trigger"], "unknown_text_retry_authorized");
    workflow = Store::new(&root.0).get(workflow.id).unwrap();
    let mut second = first.clone();
    second.request_id = Uuid::new_v4();
    assert_eq!(
        budget::reserve(&mut workflow, &second).unwrap(),
        Reserved::Fresh
    );
    assert_eq!(workflow.generations[0], saved);
    let usage = budget::usage(&workflow).unwrap();
    assert_eq!(
        usage.reserved[&ProductionUnit::Tokens],
        first.quantities[&ProductionUnit::Tokens] * 2
    );
    assert_eq!(usage.reserved[&ProductionUnit::ModelCalls], 2);
    assert!(!budget::can_reconcile(&workflow).unwrap());
    assert!(
        recovery::retry_unknown_text(&mut workflow, &[first.request_id], Uuid::new_v4(), 43)
            .is_err()
    );
}

#[test]
fn authorized_unknown_episode_review_preserves_settled_peer_and_binds_review_stage() {
    let (_root, mut workflow) = fixture(20);
    let peer = request(&mut workflow, GenerationKind::Model);
    let index = workflow
        .nodes
        .iter()
        .position(|node| node.id == peer.node_id)
        .unwrap();
    workflow.nodes[index].stage = ProductionStage::ReviewEpisode;
    workflow.nodes[index].attempt = 2;
    budget::reserve(&mut workflow, &peer).unwrap();
    budget::settle(
        &mut workflow,
        peer.request_id,
        peer.quantities.clone(),
        true,
        "d".repeat(64),
    )
    .unwrap();
    let mut failed = peer.clone();
    failed.request_id = Uuid::new_v4();
    budget::reserve(&mut workflow, &failed).unwrap();
    budget::unknown(&mut workflow, failed.request_id).unwrap();
    workflow.status = ProductionStatus::WaitingUser;
    let receipts = workflow.generations.clone();
    let usage = serde_json::to_value(budget::usage(&workflow).unwrap()).unwrap();
    recovery::retry_unknown_text(&mut workflow, &[failed.request_id], Uuid::new_v4(), 42).unwrap();
    assert_eq!(workflow.generations, receipts);
    assert_eq!(
        serde_json::to_value(budget::usage(&workflow).unwrap()).unwrap(),
        usage
    );
    assert_eq!(workflow.nodes[index].state, NodeState::Pending);
    assert_eq!(workflow.nodes[index].attempt, 2);
    assert!(budget::can_reconcile(&workflow).unwrap());
    workflow.nodes[index].history[0]
        .as_object_mut()
        .unwrap()
        .remove("stage");
    assert!(!budget::can_reconcile(&workflow).unwrap());
}

#[test]
fn exact_unknown_authorization_cannot_bypass_media_or_series_review_stages() {
    for stage in [
        ProductionStage::ReviewFrames,
        ProductionStage::ReviewVideo,
        ProductionStage::ReviewScripts,
    ] {
        let (_root, mut workflow) = fixture(20);
        let failed = request(&mut workflow, GenerationKind::Model);
        let node = workflow
            .nodes
            .iter_mut()
            .find(|node| node.id == failed.node_id)
            .unwrap();
        node.stage = stage;
        node.attempt = 1;
        budget::reserve(&mut workflow, &failed).unwrap();
        budget::unknown(&mut workflow, failed.request_id).unwrap();
        workflow.status = ProductionStatus::WaitingUser;
        let original = workflow.clone();
        assert!(
            recovery::retry_unknown_text(&mut workflow, &[failed.request_id], Uuid::new_v4(), 42)
                .is_err()
        );
        assert_eq!(workflow, original);
    }
}

#[test]
fn unknown_text_retry_requires_exact_unknown_receipts_and_a_paused_non_cancelled_writer() {
    for case in 0..7 {
        let (_root, mut workflow) = fixture(20);
        let first = request(&mut workflow, GenerationKind::Model);
        let node = workflow
            .nodes
            .iter_mut()
            .find(|node| node.id == first.node_id)
            .unwrap();
        node.attempt = 1;
        budget::reserve(&mut workflow, &first).unwrap();
        budget::unknown(&mut workflow, first.request_id).unwrap();
        workflow.status = ProductionStatus::WaitingUser;
        let mut ids = vec![first.request_id];
        match case {
            0 => ids[0] = Uuid::new_v4(),
            1 => ids.push(first.request_id),
            2 => workflow.generations[0].state = GenerationState::Reserved,
            3 => workflow.cancel_requested = true,
            4 => workflow.status = ProductionStatus::Running,
            5 => {
                workflow
                    .nodes
                    .iter_mut()
                    .find(|node| node.id == first.node_id)
                    .unwrap()
                    .attempt = 24
            }
            _ => workflow.generations[0].external_id = Some("upstream-job".into()),
        }
        let before = workflow.clone();
        assert!(recovery::retry_unknown_text(&mut workflow, &ids, Uuid::new_v4(), 42).is_err());
        assert_eq!(workflow, before);
    }
}

#[test]
fn explicit_text_retry_never_authorizes_unknown_media_or_a_modified_old_receipt() {
    for kind in [
        GenerationKind::Image,
        GenerationKind::Tts,
        GenerationKind::Music,
        GenerationKind::Video,
    ] {
        let (_root, mut workflow) = fixture(20);
        let first = request(&mut workflow, kind);
        budget::reserve(&mut workflow, &first).unwrap();
        budget::unknown(&mut workflow, first.request_id).unwrap();
        workflow.status = ProductionStatus::WaitingUser;
        assert!(
            recovery::retry_unknown_text(&mut workflow, &[first.request_id], Uuid::new_v4(), 42)
                .is_err()
        );
        assert!(!budget::can_reconcile(&workflow).unwrap());
    }
    let (_root, mut workflow) = fixture(20);
    let first = request(&mut workflow, GenerationKind::Model);
    workflow
        .nodes
        .iter_mut()
        .find(|node| node.id == first.node_id)
        .unwrap()
        .attempt = 1;
    budget::reserve(&mut workflow, &first).unwrap();
    budget::unknown(&mut workflow, first.request_id).unwrap();
    workflow.status = ProductionStatus::WaitingUser;
    recovery::retry_unknown_text(&mut workflow, &[first.request_id], Uuid::new_v4(), 42).unwrap();
    workflow.generations[0].request_digest = "c".repeat(64);
    assert!(!budget::can_reconcile(&workflow).unwrap());
}

#[test]
fn retained_unknown_text_still_counts_against_episode_call_limit() {
    let (_root, mut workflow) = fixture(20);
    workflow.input.max_model_calls_per_episode = Some(1);
    let first = request(&mut workflow, GenerationKind::Model);
    workflow
        .nodes
        .iter_mut()
        .find(|node| node.id == first.node_id)
        .unwrap()
        .attempt = 1;
    budget::reserve(&mut workflow, &first).unwrap();
    budget::unknown(&mut workflow, first.request_id).unwrap();
    workflow.status = ProductionStatus::WaitingUser;
    recovery::retry_unknown_text(&mut workflow, &[first.request_id], Uuid::new_v4(), 42).unwrap();
    workflow.status = ProductionStatus::Running;
    let node = workflow
        .nodes
        .iter_mut()
        .find(|node| node.id == first.node_id)
        .unwrap();
    node.state = NodeState::Running;
    node.grant = Uuid::new_v4();
    node.attempt = 2;
    let mut second = first.clone();
    second.request_id = Uuid::new_v4();
    assert!(
        budget::reserve(&mut workflow, &second)
            .unwrap_err()
            .to_string()
            .contains("episode_1_model_calls")
    );
    assert_eq!(workflow.generations.len(), 1);
}

#[test]
fn async_receipt_reconciliation_never_authorizes_a_second_purchase() {
    for kind in [GenerationKind::Music, GenerationKind::Video] {
        let (_root, mut workflow) = fixture(20);
        let saved = request(&mut workflow, kind);
        budget::reserve(&mut workflow, &saved).unwrap();
        assert!(budget::can_reconcile(&workflow).unwrap());
        let mut another = saved.clone();
        another.request_id = Uuid::new_v4();
        assert!(budget::reserve(&mut workflow, &another).is_err());
        assert!(matches!(
            budget::reserve(&mut workflow, &saved).unwrap(),
            Reserved::Existing(_)
        ));
        budget::submitted(&mut workflow, saved.request_id, "original-provider-job").unwrap();
        assert!(
            budget::usage(&workflow)
                .unwrap()
                .unknown_requests
                .is_empty()
        );
        assert_eq!(workflow.generations.len(), 1);
    }
}

#[test]
fn unknown_outcomes_and_non_async_reservations_cannot_resume_into_reconciliation() {
    for kind in [
        GenerationKind::Model,
        GenerationKind::Image,
        GenerationKind::Tts,
        GenerationKind::Music,
        GenerationKind::Video,
    ] {
        let (_root, mut workflow) = fixture(20);
        let saved = request(&mut workflow, kind);
        budget::reserve(&mut workflow, &saved).unwrap();
        if matches!(kind, GenerationKind::Music | GenerationKind::Video) {
            budget::unknown(&mut workflow, saved.request_id).unwrap();
        }
        assert!(!budget::can_reconcile(&workflow).unwrap());
    }
}

#[test]
fn an_unconfirmed_request_blocks_further_spending_in_every_resource_class() {
    let (_root, mut workflow) = fixture(20);
    let first = request(&mut workflow, GenerationKind::Image);
    budget::reserve(&mut workflow, &first).unwrap();
    for kind in [
        GenerationKind::Image,
        GenerationKind::Model,
        GenerationKind::Music,
        GenerationKind::Video,
    ] {
        let next = request(&mut workflow, kind);
        assert!(
            budget::reserve(&mut workflow, &next)
                .unwrap_err()
                .to_string()
                .contains("uncertain")
        );
    }
}

#[test]
fn submitted_videos_keep_their_reservation_and_can_coexist_with_other_work() {
    let (_root, mut workflow) = fixture(20);
    let first = request(&mut workflow, GenerationKind::Video);
    budget::reserve(&mut workflow, &first).unwrap();
    budget::submitted(&mut workflow, first.request_id, "upstream-video-123").unwrap();
    budget::submitted(&mut workflow, first.request_id, "upstream-video-123").unwrap();
    assert!(budget::submitted(&mut workflow, first.request_id, "wrong-job").is_err());
    let next = request(&mut workflow, GenerationKind::Music);
    assert_eq!(
        budget::reserve(&mut workflow, &next).unwrap(),
        Reserved::Fresh
    );
    let usage = budget::usage(&workflow).unwrap();
    assert_eq!(usage.reserved[&ProductionUnit::VideoSeconds], 10);
    assert_eq!(usage.used.get(&ProductionUnit::VideoSeconds), None);
    assert_eq!(
        workflow.generations[0].external_id.as_deref(),
        Some("upstream-video-123")
    );
}

#[test]
fn background_workers_retain_their_grant_while_the_controller_waits_for_external_work() {
    let (_root, mut workflow) = fixture(20);
    let request = request(&mut workflow, GenerationKind::Video);
    let node = workflow
        .nodes
        .iter_mut()
        .find(|node| node.id == request.node_id)
        .unwrap();
    node.state = NodeState::WaitingExternal;
    assert_eq!(
        budget::reserve(&mut workflow, &request).unwrap(),
        Reserved::Fresh
    );
    budget::submitted(&mut workflow, request.request_id, "existing-background-job").unwrap();
    let mut after_completion = request.clone();
    after_completion.request_id = Uuid::new_v4();
    workflow
        .nodes
        .iter_mut()
        .find(|node| node.id == request.node_id)
        .unwrap()
        .state = NodeState::Completed;
    assert!(budget::reserve(&mut workflow, &after_completion).is_err());
}

#[test]
fn every_item_in_a_batch_consumes_the_same_cumulative_quota() {
    let (_root, mut workflow) = fixture(20);
    workflow
        .input
        .limits
        .insert(ProductionUnit::Images, Some(2));
    for _ in 0..2 {
        let image = request(&mut workflow, GenerationKind::Image);
        budget::reserve(&mut workflow, &image).unwrap();
        budget::settle(
            &mut workflow,
            image.request_id,
            image.quantities.clone(),
            true,
            "d".repeat(64),
        )
        .unwrap();
    }
    let third = request(&mut workflow, GenerationKind::Image);
    assert!(
        budget::reserve(&mut workflow, &third)
            .unwrap_err()
            .to_string()
            .contains("budget_limited")
    );
    assert_eq!(workflow.generations.len(), 2);
}

#[test]
fn wrong_stage_wrong_units_zero_reservation_or_missing_grant_cannot_spend() {
    let (_root, mut workflow) = fixture(20);
    let mut request = request(&mut workflow, GenerationKind::Video);
    request.quantities = [(ProductionUnit::Images, 1)].into();
    assert!(budget::reserve(&mut workflow, &request).is_err());
    request.quantities = [
        (ProductionUnit::VideoCalls, 1),
        (ProductionUnit::VideoSeconds, 0),
    ]
    .into();
    assert!(budget::reserve(&mut workflow, &request).is_err());
    request.quantities.insert(ProductionUnit::VideoSeconds, 10);
    let node = workflow
        .nodes
        .iter_mut()
        .find(|node| node.id == request.node_id)
        .unwrap();
    node.stage = ProductionStage::ReviewVideo;
    assert!(budget::reserve(&mut workflow, &request).is_err());
    let node = workflow
        .nodes
        .iter_mut()
        .find(|node| node.id == request.node_id)
        .unwrap();
    node.stage = ProductionStage::GenerateVideo;
    node.grant = Uuid::nil();
    assert!(budget::reserve(&mut workflow, &request).is_err());
    assert!(workflow.generations.is_empty());
}

#[test]
fn uncertain_failure_retains_full_quota_instead_of_becoming_zero_cost() {
    let (_root, mut workflow) = fixture(20);
    let request = request(&mut workflow, GenerationKind::Music);
    budget::reserve(&mut workflow, &request).unwrap();
    budget::unknown(&mut workflow, request.request_id).unwrap();
    let usage = budget::usage(&workflow).unwrap();
    assert_eq!(workflow.status, ProductionStatus::WaitingUser);
    assert_eq!(usage.reserved, request.quantities);
    assert!(usage.used.is_empty());
    assert_eq!(usage.unknown_requests, vec![request.request_id]);
}

#[test]
fn known_failure_preserves_metering_but_still_consumes_a_generation_attempt() {
    let (_root, mut workflow) = fixture(20);
    let request = request(&mut workflow, GenerationKind::Video);
    budget::reserve(&mut workflow, &request).unwrap();
    let actual = request.kind.units().iter().map(|unit| (*unit, 0)).collect();
    budget::settle(
        &mut workflow,
        request.request_id,
        actual,
        false,
        "d".repeat(64),
    )
    .unwrap();
    let usage = budget::usage(&workflow).unwrap();
    assert_eq!(usage.used[&ProductionUnit::VideoCalls], 1);
    assert_eq!(usage.used[&ProductionUnit::VideoSeconds], 0);
    assert_eq!(
        workflow.generations[0].actual.as_ref().unwrap()[&ProductionUnit::VideoCalls],
        0
    );
}

#[test]
fn settlement_is_idempotent_but_changed_metering_or_evidence_is_rejected() {
    let (_root, mut workflow) = fixture(20);
    let request = request(&mut workflow, GenerationKind::Tts);
    budget::reserve(&mut workflow, &request).unwrap();
    for _ in 0..2 {
        budget::settle(
            &mut workflow,
            request.request_id,
            request.quantities.clone(),
            true,
            "d".repeat(64),
        )
        .unwrap();
    }
    assert!(
        budget::settle(
            &mut workflow,
            request.request_id,
            request.quantities,
            true,
            "e".repeat(64)
        )
        .is_err()
    );
    assert_eq!(
        budget::usage(&workflow).unwrap().used[&ProductionUnit::TtsCharacters],
        10
    );
}

fn validation_failure() -> Value {
    json!({"ok":false,"error":{"code":"production_step_invalid",
        "message":"production_model_validation_failed: production_evidence_not_in_scene"}})
}

#[test]
fn settled_review_validation_at_attempt_eight_retries_without_resetting_history_or_usage() {
    let (_root, mut workflow) = fixture(20);
    let failed = request(&mut workflow, GenerationKind::Model);
    let index = workflow
        .nodes
        .iter()
        .position(|node| node.id == failed.node_id)
        .unwrap();
    workflow.nodes[index].stage = ProductionStage::ReviewEpisode;
    workflow.nodes[index].attempt = 8;
    budget::reserve(&mut workflow, &failed).unwrap();
    budget::settle(
        &mut workflow,
        failed.request_id,
        failed.quantities.clone(),
        true,
        "d".repeat(64),
    )
    .unwrap();
    let receipts = workflow.generations.clone();
    assert!(validation::retry(&mut workflow, failed.node_id, &validation_failure()).unwrap());
    assert_eq!(workflow.nodes[index].state, NodeState::Pending);
    assert_eq!(workflow.nodes[index].attempt, 8);
    assert_eq!(workflow.nodes[index].history[0]["attempt"], 8);
    assert_eq!(workflow.generations, receipts);
    workflow.nodes[index].state = NodeState::Running;
    workflow.nodes[index].attempt = 24;
    assert!(!validation::retry(&mut workflow, failed.node_id, &validation_failure()).unwrap());
    assert_eq!(workflow.generations, receipts);
}

#[test]
fn episode_text_limits_preserve_other_text_and_media_limits() {
    assert_eq!(
        engine::node_attempt_limit(ProductionStage::ReviewScripts),
        8
    );
    assert_eq!(
        engine::node_attempt_limit(ProductionStage::WriteEpisode),
        24
    );
    assert_eq!(
        engine::node_attempt_limit(ProductionStage::ReviewEpisode),
        24
    );
    for stage in [
        ProductionStage::GenerateFrames,
        ProductionStage::ReviewFrames,
        ProductionStage::GenerateVideo,
    ] {
        assert_eq!(engine::node_attempt_limit(stage), 4);
    }
}

#[test]
fn invalid_settled_text_retries_with_history_without_erasing_usage_or_grant_limits() {
    let (_root, mut workflow) = fixture(20);
    let reservation = request(&mut workflow, GenerationKind::Model);
    budget::reserve(&mut workflow, &reservation).unwrap();
    budget::settle(
        &mut workflow,
        reservation.request_id,
        reservation.quantities.clone(),
        true,
        "d".repeat(64),
    )
    .unwrap();
    let index = workflow
        .nodes
        .iter()
        .position(|node| node.id == reservation.node_id)
        .unwrap();
    workflow.nodes[index].attempt = 1;
    let before = workflow.clone();
    assert!(validation::retry(&mut workflow, reservation.node_id, &validation_failure()).unwrap());
    let node = &workflow.nodes[index];
    assert_eq!(node.state, NodeState::Pending);
    assert_eq!(node.attempt, 1);
    assert_eq!(node.history[0]["result"], validation_failure());
    assert_eq!(
        node.history[0]["inputDigest"],
        before.nodes[index].input_digest
    );
    assert!(node.grant.is_nil());
    assert_eq!(workflow.generations, before.generations);
    assert_eq!(
        serde_json::to_value(budget::usage(&workflow).unwrap()).unwrap(),
        serde_json::to_value(budget::usage(&before).unwrap()).unwrap()
    );
    assert_eq!(workflow.input, before.input);
}

#[test]
fn invalid_text_with_reserved_submitted_or_unknown_requests_never_buys_a_retry() {
    for state in [
        GenerationState::Reserved,
        GenerationState::Submitted,
        GenerationState::Unknown,
    ] {
        let (_root, mut workflow) = fixture(20);
        let reservation = request(&mut workflow, GenerationKind::Model);
        budget::reserve(&mut workflow, &reservation).unwrap();
        workflow.generations[0].state = state;
        let before = workflow.clone();
        assert!(
            validation::retry(&mut workflow, reservation.node_id, &validation_failure()).is_err()
        );
        assert_eq!(workflow, before);
    }
}

#[test]
fn validation_retries_are_bounded_and_do_not_retry_media_cancellation_or_transport_errors() {
    for scenario in ["limit", "media", "cancelled", "transport", "paused"] {
        let (_root, mut workflow) = fixture(20);
        let reservation = request(&mut workflow, GenerationKind::Model);
        let index = workflow
            .nodes
            .iter()
            .position(|node| node.id == reservation.node_id)
            .unwrap();
        let mut error = validation_failure();
        match scenario {
            "limit" => {
                workflow.nodes[index].attempt =
                    engine::node_attempt_limit(workflow.nodes[index].stage)
            }
            "media" => workflow.nodes[index].stage = ProductionStage::GenerateVideo,
            "cancelled" => workflow.cancel_requested = true,
            "transport" => error["error"]["message"] = json!("production_host_request_failed"),
            "paused" => workflow.status = ProductionStatus::WaitingUser,
            _ => unreachable!(),
        }
        let before = workflow.clone();
        assert!(!validation::retry(&mut workflow, reservation.node_id, &error).unwrap());
        assert_eq!(workflow, before);
    }
}

#[test]
fn exceeding_estimate_is_recorded_truthfully_and_pauses_further_spending() {
    let (_root, mut workflow) = fixture(20);
    workflow
        .input
        .limits
        .insert(ProductionUnit::TtsCharacters, Some(10));
    let request = request(&mut workflow, GenerationKind::Tts);
    budget::reserve(&mut workflow, &request).unwrap();
    budget::settle(
        &mut workflow,
        request.request_id,
        [(ProductionUnit::TtsCharacters, 12)].into(),
        true,
        "d".repeat(64),
    )
    .unwrap();
    assert_eq!(workflow.status, ProductionStatus::BudgetLimited);
    assert_eq!(
        budget::usage(&workflow).unwrap().used[&ProductionUnit::TtsCharacters],
        12
    );
}

#[test]
fn cancellation_stops_new_requests_and_allows_late_metering() {
    let (_root, mut workflow) = fixture(20);
    let first = request(&mut workflow, GenerationKind::Video);
    budget::reserve(&mut workflow, &first).unwrap();
    workflow.cancel_requested = true;
    workflow.status = ProductionStatus::Cancelled;
    let next = request(&mut workflow, GenerationKind::Image);
    assert!(budget::reserve(&mut workflow, &next).is_err());
    budget::settle(
        &mut workflow,
        first.request_id,
        first.quantities,
        true,
        "d".repeat(64),
    )
    .unwrap();
    assert_eq!(workflow.status, ProductionStatus::Cancelled);
    assert_eq!(
        budget::usage(&workflow).unwrap().used[&ProductionUnit::VideoSeconds],
        10
    );
}

#[test]
fn raising_total_limits_retains_initial_input_and_usage_and_does_not_resume() {
    let (_root, mut workflow) = fixture(20);
    let request = request(&mut workflow, GenerationKind::Image);
    budget::reserve(&mut workflow, &request).unwrap();
    budget::settle(
        &mut workflow,
        request.request_id,
        request.quantities,
        true,
        "d".repeat(64),
    )
    .unwrap();
    let original = workflow.input.clone();
    let expected = original.limits.clone();
    let mut next = expected.clone();
    next.insert(ProductionUnit::Images, Some(200));
    workflow.status = ProductionStatus::BudgetLimited;
    let id = Uuid::new_v4();
    for _ in 0..2 {
        budget::increase(&mut workflow, id, 2, &expected, next.clone()).unwrap();
    }
    assert_eq!(workflow.input, original);
    assert_eq!(workflow.status, ProductionStatus::WaitingUser);
    assert_eq!(workflow.budget_changes.len(), 1);
    assert_eq!(
        budget::usage(&workflow).unwrap().used[&ProductionUnit::Images],
        1
    );
    assert!(budget::increase(&mut workflow, Uuid::new_v4(), 3, &expected, next).is_err());
}

#[test]
fn late_model_settlement_checks_its_episode_and_preserves_the_actual_excess() {
    let (_root, mut workflow) = fixture(80);
    workflow.input.max_model_calls_per_episode = Some(1);
    let first = request(&mut workflow, GenerationKind::Model);
    budget::reserve(&mut workflow, &first).unwrap();
    let mut actual = first.quantities.clone();
    actual.insert(ProductionUnit::ModelCalls, 2);
    budget::settle(
        &mut workflow,
        first.request_id,
        actual,
        true,
        "a".repeat(64),
    )
    .unwrap();
    assert_eq!(workflow.status, ProductionStatus::BudgetLimited);
    assert_eq!(
        budget::usage(&workflow).unwrap().used[&ProductionUnit::ModelCalls],
        2
    );
    assert!(
        workflow
            .nodes
            .iter()
            .filter(|node| node.episode == Some(80))
            .all(|node| node.state == NodeState::Pending)
    );
}

#[test]
fn local_parameter_rejections_are_recorded_without_inventing_a_free_upstream_outcome() {
    let (_root, mut workflow) = fixture(20);
    let request = request(&mut workflow, GenerationKind::Model);
    budget::reserve(&mut workflow, &request).unwrap();
    let error = willdeep_core::mcp::HostRequestError::invalid_params("unknownModel");
    let saved =
        host::local_rejection(GenerationKind::Model, &request.request_digest, &error).unwrap();
    assert_eq!(saved["response"]["error"]["dispatchStarted"], false);
    budget::settle(
        &mut workflow,
        request.request_id,
        serde_json::from_value(saved["actual"].clone()).unwrap(),
        false,
        "e".repeat(64),
    )
    .unwrap();
    let usage = budget::usage(&workflow).unwrap();
    assert_eq!(usage.used[&ProductionUnit::ModelCalls], 1);
    assert_eq!(usage.used[&ProductionUnit::Tokens], 0);
    assert!(usage.unknown_requests.is_empty());
    let transport = willdeep_core::mcp::HostRequestError::failed("HTTP 500");
    assert!(
        host::local_rejection(GenerationKind::Model, &request.request_digest, &transport).is_none()
    );
    let unknown = willdeep_core::mcp::HostRequestError::invalid_params("provider_error");
    assert!(
        host::local_rejection(GenerationKind::Model, &request.request_digest, &unknown).is_none()
    );
}

#[test]
fn explicit_unlimited_media_still_tracks_actual_usage_and_preserves_failed_attempts() {
    let (_root, mut workflow) = fixture(80);
    workflow.input.limits.insert(ProductionUnit::Images, None);
    workflow.input.validate().unwrap();
    let mut image = request(&mut workflow, GenerationKind::Image);
    image.quantities.insert(ProductionUnit::Images, 10_000);
    budget::reserve(&mut workflow, &image).unwrap();
    budget::settle(
        &mut workflow,
        image.request_id,
        image.quantities,
        true,
        "e".repeat(64),
    )
    .unwrap();
    let usage = budget::usage(&workflow).unwrap();
    assert_eq!(usage.limits[&ProductionUnit::Images], None);
    assert_eq!(usage.remaining[&ProductionUnit::Images], None);
    assert_eq!(usage.used[&ProductionUnit::Images], 10_000);
    assert_eq!(workflow.status, ProductionStatus::Running);
    let expected = usage.limits;
    let mut reduced = expected.clone();
    reduced.insert(ProductionUnit::Images, Some(20_000));
    assert!(budget::increase(&mut workflow, Uuid::new_v4(), 2, &expected, reduced).is_err());
    let json = serde_json::to_value(&workflow).unwrap();
    assert!(json["input"]["limits"]["images"].is_null());
    assert_eq!(
        serde_json::from_value::<ProductionWorkflow>(json).unwrap(),
        workflow
    );
}

#[test]
fn episode_call_cap_includes_all_review_nodes_and_does_not_repeat_existing_request() {
    let (_root, mut workflow) = fixture(80);
    workflow.input.max_model_calls_per_episode = Some(1);
    let first = request(&mut workflow, GenerationKind::Model);
    let episode = workflow
        .nodes
        .iter()
        .find(|node| node.id == first.node_id)
        .unwrap()
        .episode;
    budget::reserve(&mut workflow, &first).unwrap();
    budget::settle(
        &mut workflow,
        first.request_id,
        first.quantities.clone(),
        false,
        "f".repeat(64),
    )
    .unwrap();
    assert!(matches!(
        budget::reserve(&mut workflow, &first).unwrap(),
        Reserved::Existing(_)
    ));
    let mut review = first.clone();
    review.request_id = Uuid::new_v4();
    let node = workflow
        .nodes
        .iter_mut()
        .find(|node| node.stage == ProductionStage::ReviewEpisode && node.episode == episode)
        .unwrap();
    node.state = NodeState::Running;
    node.grant = Uuid::new_v4();
    node.input_digest = "a".repeat(64);
    review.node_id = node.id;
    assert!(
        budget::reserve(&mut workflow, &review)
            .unwrap_err()
            .to_string()
            .contains("episode_1_model_calls")
    );
    let other = workflow
        .nodes
        .iter_mut()
        .find(|node| node.stage == ProductionStage::WriteEpisode && node.episode == Some(2))
        .unwrap();
    other.state = NodeState::Running;
    other.grant = Uuid::new_v4();
    other.input_digest = "a".repeat(64);
    review.node_id = other.id;
    budget::reserve(&mut workflow, &review).unwrap();
}

#[test]
fn corrupt_and_mismatched_archives_are_not_replaced_as_empty() {
    let (root, workflow) = fixture(20);
    let file = root
        .0
        .join("runtime/productions")
        .join(format!("{}.json", workflow.id));
    std::fs::write(&file, "{").unwrap();
    assert!(Store::new(&root.0).get(workflow.id).is_err());
    assert!(Store::new(&root.0).list().is_err());
    assert!(
        Store::new(&root.0)
            .create(workflow.id, workflow.input.clone())
            .is_err()
    );
    let mut wrong = workflow.clone();
    wrong.id = Uuid::new_v4();
    std::fs::write(file, serde_json::to_vec(&wrong).unwrap()).unwrap();
    assert!(Store::new(&root.0).get(workflow.id).is_err());
}

#[test]
fn failed_state_edit_is_atomic_and_never_partially_charges_the_budget() {
    let (root, workflow) = fixture(20);
    let result = Store::new(&root.0).update(workflow.id, |saved| -> Result<()> {
        saved.input.limits.clear();
        anyhow::bail!("deliberate fixture failure")
    });
    assert!(result.is_err());
    assert_eq!(
        Store::new(&root.0).get(workflow.id).unwrap().input,
        workflow.input
    );
}

#[test]
fn execution_lease_prevents_two_controllers_from_dispatching_the_same_task() {
    let (root, workflow) = fixture(20);
    let lease = Store::new(&root.0).lock(workflow.id, true).unwrap();
    assert!(Store::new(&root.0).lock(workflow.id, true).is_err());
    drop(lease);
    assert!(Store::new(&root.0).lock(workflow.id, true).is_ok());
}

#[test]
fn node_dispatch_waits_for_dependencies_and_reuses_the_same_grant_after_interruption() {
    let (root, workflow) = fixture(80);
    let first = engine::prepare(&root.0, workflow.id).unwrap();
    assert_eq!(first.stage, ProductionStage::Preflight);
    assert_eq!(first.attempt, 1);
    let recovered = engine::prepare(&root.0, workflow.id).unwrap();
    assert_eq!(recovered, first);
    Store::new(&root.0)
        .update(workflow.id, |saved| {
            engine::finish(
                saved,
                first.id,
                json!({"inputDigest":first.input_digest,"state":"completed"}),
            )
        })
        .unwrap();
    let next = engine::prepare(&root.0, workflow.id).unwrap();
    assert_eq!(next.stage, ProductionStage::SeriesPlan);
    assert_ne!(next.grant, first.grant);
    assert_ne!(next.input_digest, first.input_digest);
}

#[test]
fn pending_upstream_job_polls_the_same_node_instead_of_creating_a_second_submission() {
    let (root, workflow) = fixture(20);
    let first = engine::prepare(&root.0, workflow.id).unwrap();
    Store::new(&root.0).update(workflow.id, |saved| engine::finish(saved, first.id,
        json!({"inputDigest":first.input_digest,"state":"waiting_external","jobID":"same-upstream"}))).unwrap();
    let polled = engine::prepare(&root.0, workflow.id).unwrap();
    assert_eq!(polled.id, first.id);
    assert_eq!(polled.grant, first.grant);
    assert_eq!(polled.arguments, first.arguments);
    assert_eq!(polled.attempt, 1);
}

#[test]
fn outdated_node_result_cannot_advance_the_series() {
    let (root, workflow) = fixture(20);
    let first = engine::prepare(&root.0, workflow.id).unwrap();
    let result = Store::new(&root.0).update(workflow.id, |saved| {
        engine::finish(
            saved,
            first.id,
            json!({"inputDigest":"wrong-version","state":"completed"}),
        )
    });
    assert!(result.is_err());
    assert_eq!(
        Store::new(&root.0).get(workflow.id).unwrap().nodes[0].state,
        NodeState::Running
    );
}

fn review_node(workflow: &mut ProductionWorkflow) -> (Uuid, Uuid) {
    let target = workflow
        .nodes
        .iter_mut()
        .find(|node| node.stage == ProductionStage::WriteEpisode)
        .unwrap();
    target.state = NodeState::Completed;
    target.attempt = 1;
    target.result = Some(json!({"candidate":"old version"}));
    let target_id = target.id;
    let review = workflow
        .nodes
        .iter_mut()
        .find(|node| node.stage == ProductionStage::ReviewEpisode)
        .unwrap();
    review.state = NodeState::Running;
    review.attempt = 1;
    review.input_digest = "a".repeat(64);
    review.grant = Uuid::new_v4();
    (target_id, review.id)
}

#[test]
fn review_without_exact_episode_coverage_cannot_unlock_paid_media() {
    let (_root, mut workflow) = fixture(80);
    let (_, review) = review_node(&mut workflow);
    let result = json!({"inputDigest":"a".repeat(64),"state":"completed",
        "review":{"inputDigest":"a".repeat(64),"coverage":[1,2,3],"mustFix":0,"verdict":"pass"}});
    assert!(engine::finish(&mut workflow, review, result).is_err());
    assert_eq!(
        workflow
            .nodes
            .iter()
            .find(|node| node.id == review)
            .unwrap()
            .state,
        NodeState::Running
    );
}

#[test]
fn mandatory_review_triggers_bounded_rework_and_preserves_old_candidates() {
    let (_root, mut workflow) = fixture(20);
    let (target, review) = review_node(&mut workflow);
    let result = json!({"inputDigest":"a".repeat(64),"state":"completed","reworkTarget":target,
        "review":{"inputDigest":"a".repeat(64),"coverage":[1],"mustFix":1,"verdict":"block",
            "issues":[{"reason":"father knows an undisclosed fact"}]}});
    assert!(!engine::finish(&mut workflow, review, result).unwrap());
    let target = workflow
        .nodes
        .iter()
        .find(|node| node.id == target)
        .unwrap();
    assert_eq!(target.state, NodeState::Pending);
    assert_eq!(target.history[0]["result"]["candidate"], "old version");
    assert!(target.grant.is_nil());
    assert_eq!(
        workflow
            .nodes
            .iter()
            .find(|node| node.id == review)
            .unwrap()
            .state,
        NodeState::Pending
    );
    assert_eq!(workflow.status, ProductionStatus::Running);
}

#[test]
fn film_and_series_reviews_can_retake_video_only_inside_their_coverage() {
    for stage in [ProductionStage::ReviewFilm, ProductionStage::ReviewSeries] {
        let (_root, mut workflow) = fixture(20);
        for node in &mut workflow.nodes {
            node.state = NodeState::Completed;
            node.attempt = 1;
            node.input_digest = "a".repeat(64);
        }
        let target = workflow
            .nodes
            .iter()
            .find(|node| node.stage == ProductionStage::GenerateVideo && node.episode == Some(1))
            .unwrap()
            .id;
        let unrelated = workflow
            .nodes
            .iter()
            .find(|node| node.stage == ProductionStage::GenerateVideo && node.episode == Some(20))
            .unwrap()
            .id;
        let review = workflow
            .nodes
            .iter_mut()
            .find(|node| node.stage == stage)
            .unwrap();
        review.state = NodeState::Running;
        review.grant = Uuid::new_v4();
        let review_id = review.id;
        let coverage = if stage == ProductionStage::ReviewFilm {
            vec![1]
        } else {
            (1..=8).collect()
        };
        let result = json!({"inputDigest":"a".repeat(64),"state":"completed","reworkTarget":target,
            "review":{"inputDigest":"a".repeat(64),"coverage":coverage,"mustFix":1,"verdict":"block"}});
        let mut denied = workflow.clone();
        let mut wrong = result.clone();
        wrong["reworkTarget"] = json!(unrelated);
        assert!(engine::finish(&mut denied, review_id, wrong).is_err());
        assert!(!engine::finish(&mut workflow, review_id, result).unwrap());
        assert_eq!(
            workflow
                .nodes
                .iter()
                .find(|node| node.id == target)
                .unwrap()
                .state,
            NodeState::Pending
        );
        assert_eq!(
            workflow
                .nodes
                .iter()
                .find(|node| node.id == unrelated)
                .unwrap()
                .state,
            NodeState::Completed
        );
        assert_eq!(
            workflow
                .nodes
                .iter()
                .find(|node| node.stage == ProductionStage::WriteEpisode && node.episode == Some(1))
                .unwrap()
                .state,
            NodeState::Completed
        );
    }
}

#[test]
fn review_cannot_repair_an_unrelated_episode_or_an_unfinished_external_request() {
    let (_root, mut workflow) = fixture(20);
    let (target, review) = review_node(&mut workflow);
    let wrong_target = workflow
        .nodes
        .iter()
        .find(|node| node.stage == ProductionStage::WriteEpisode && node.episode == Some(2))
        .unwrap()
        .id;
    let mut result = json!({"inputDigest":"a".repeat(64),"state":"completed","reworkTarget":wrong_target,
        "review":{"inputDigest":"a".repeat(64),"coverage":[1],"mustFix":1,"verdict":"block"}});
    assert!(engine::finish(&mut workflow, review, result.clone()).is_err());
    let request = request(&mut workflow, GenerationKind::Video);
    budget::reserve(&mut workflow, &request).unwrap();
    result["reworkTarget"] = json!(target);
    assert!(
        engine::finish(&mut workflow, review, result)
            .unwrap_err()
            .to_string()
            .contains("inflight")
    );
}

#[test]
fn a_three_episode_export_cannot_complete_an_eighty_episode_production() {
    let (_root, mut workflow) = fixture(80);
    for node in &mut workflow.nodes {
        node.state = NodeState::Completed;
    }
    let deliver = workflow.nodes.last_mut().unwrap();
    deliver.state = NodeState::Running;
    deliver.input_digest = "a".repeat(64);
    let id = deliver.id;
    let result = json!({"inputDigest":"a".repeat(64),"state":"completed",
        "delivery":{"episodes":[{"episodeOrder":1},{"episodeOrder":2},{"episodeOrder":3}]}});
    assert!(
        engine::finish(&mut workflow, id, result)
            .unwrap_err()
            .to_string()
            .contains("incomplete")
    );
    assert_eq!(workflow.status, ProductionStatus::Running);
}

#[test]
fn passing_the_script_does_not_complete_the_production_or_bypass_final_file_provenance() {
    let (_root, mut workflow) = fixture(20);
    let (_, review) = review_node(&mut workflow);
    engine::finish(
        &mut workflow,
        review,
        json!({"inputDigest":"a".repeat(64),"state":"completed",
        "review":{"inputDigest":"a".repeat(64),"coverage":[1],"mustFix":0,"verdict":"pass"}}),
    )
    .unwrap();
    assert_eq!(workflow.status, ProductionStatus::Running);
    for node in &mut workflow.nodes {
        node.state = NodeState::Completed;
    }
    let deliver = workflow.nodes.last_mut().unwrap();
    deliver.state = NodeState::Running;
    deliver.input_digest = "a".repeat(64);
    let id = deliver.id;
    let episodes: Vec<_> = (1..=20)
        .map(|order| {
            json!({"episodeOrder":order,
        "sha256":"b".repeat(64),"bytes":1_000,"durationMs":90_000})
        })
        .collect();
    let result = json!({"inputDigest":"a".repeat(64),"state":"completed",
        "delivery":{"episodes":episodes,"manifestSHA256":"c".repeat(64)}});
    assert!(
        engine::finish(&mut workflow, id, result)
            .unwrap_err()
            .to_string()
            .contains("composition_mismatch")
    );
}

#[test]
fn public_progress_omits_grants_arguments_and_private_provider_receipts() {
    let (root, workflow) = fixture(20);
    let node = engine::prepare(&root.0, workflow.id).unwrap();
    let saved = Store::new(&root.0).get(workflow.id).unwrap();
    let public = public(saved).unwrap().to_string();
    assert!(!public.contains(&node.grant.to_string()));
    assert!(!public.contains("\"arguments\""));
    assert!(public.contains("\"episode_count\":20"));
}

#[test]
fn final_delivery_requires_all_episode_proofs_and_downloadable_manifest_and_records() {
    let (_root, mut workflow) = fixture(20);
    for node in &mut workflow.nodes {
        node.state = NodeState::Completed;
    }
    let episodes: Vec<_> = (1..=20).map(|order| json!({"episodeOrder":order,"sha256":"b".repeat(64),
        "bytes":1000,"durationMs":90000,"sourceDigest":"d".repeat(64),"filename":format!("episode-{order:03}-{}.mp4", "b".repeat(16))})).collect();
    for node in &mut workflow.nodes {
        if node.stage == ProductionStage::ComposeEpisode {
            node.result =
                Some(json!({"artifact":episodes[usize::from(node.episode.unwrap()) - 1]}));
        }
    }
    let deliver = workflow.nodes.last_mut().unwrap();
    deliver.state = NodeState::Running;
    deliver.input_digest = "a".repeat(64);
    let id = deliver.id;
    let mut result = json!({"inputDigest":"a".repeat(64),"state":"completed","delivery":{
        "episodes":episodes,"manifestSHA256":"c".repeat(64),"manifestBytes":100,
        "manifestFilename":format!("manifest-{}.json","c".repeat(24))}});
    assert!(
        engine::finish(&mut workflow.clone(), id, result.clone())
            .unwrap_err()
            .to_string()
            .contains("records_required")
    );
    result["delivery"]["records"] = json!({"sha256":"e".repeat(64),"bytes":100,"filename":format!("records-{}.json","e".repeat(24))});
    let mut missing_source = workflow.clone();
    missing_source
        .nodes
        .iter_mut()
        .find(|node| node.stage == ProductionStage::ComposeEpisode && node.episode == Some(1))
        .unwrap()
        .result
        .as_mut()
        .unwrap()["artifact"]["sourceDigest"] = Value::Null;
    let mut missing_proof = result.clone();
    missing_proof["delivery"]["episodes"][0]["sourceDigest"] = Value::Null;
    assert!(
        engine::finish(&mut missing_source, id, missing_proof)
            .unwrap_err()
            .to_string()
            .contains("source_digest_required")
    );
    let mut changed = result.clone();
    changed["delivery"]["episodes"][19]["durationMs"] = json!(1);
    assert!(
        engine::finish(&mut workflow.clone(), id, changed)
            .unwrap_err()
            .to_string()
            .contains("composition_mismatch")
    );
    engine::finish(&mut workflow, id, result).unwrap();
    assert_eq!(workflow.status, ProductionStatus::Completed);
}

#[test]
fn persisted_provider_response_reconciles_after_a_crash_without_another_paid_request() {
    let (root, mut workflow) = fixture(20);
    let request = request(&mut workflow, GenerationKind::Image);
    let id = workflow.id;
    Store::new(&root.0)
        .update(id, |saved| {
            *saved = workflow;
            budget::reserve(saved, &request)
        })
        .unwrap();
    Store::new(&root.0)
        .save_receipt(
            id,
            request.request_id,
            &json!({
                "requestDigest":request.request_digest,"actual":request.quantities,"succeeded":true,
                "response":{"filePath":"fixture-only"}
            }),
        )
        .unwrap();
    for _ in 0..2 {
        host::reconcile(&root.0, id).unwrap();
    }
    let recovered = Store::new(&root.0).get(id).unwrap();
    assert_eq!(recovered.generations.len(), 1);
    assert_eq!(recovered.generations[0].state, GenerationState::Settled);
    assert_eq!(
        budget::usage(&recovered).unwrap().used[&ProductionUnit::Images],
        1
    );
}

#[tokio::test]
async fn forged_host_grant_is_rejected_before_any_model_dispatch() {
    use willdeep_core::plugin::{PluginHostRequests, PluginPermission, PluginRequestContext};
    let (root, workflow) = fixture(20);
    let node = engine::prepare(&root.0, workflow.id).unwrap();
    let host = host::Host {
        home: root.0.clone(),
        workflow_id: workflow.id,
        inner: crate::plugin_host_requests::HostRequests::new(
            root.0.clone(),
            root.0.join("config.toml"),
            std::sync::Arc::new(std::sync::RwLock::new(vec![root.0.clone()])),
        ),
    };
    let context = PluginRequestContext {
        plugin_id: PLUGIN.into(),
        permissions: vec![PluginPermission::AiChat],
    };
    let mut forged = node.arguments;
    forged["grant"] = json!(Uuid::new_v4());
    assert!(
        host.handle(&context, host::AUTHORIZE, forged)
            .await
            .is_err()
    );
    assert!(
        host.handle(
            &context,
            willdeep_core::plugin::host_requests::AI_COMPLETE,
            json!({"messages":[{"role":"user","content":"must not dispatch"}]})
        )
        .await
        .is_err()
    );
    assert!(
        Store::new(&root.0)
            .get(workflow.id)
            .unwrap()
            .generations
            .is_empty()
    );
}

#[test]
fn production_queries_are_read_only_and_paid_starts_use_runtime_idempotency() {
    for operation in ["production.status", "production.list"] {
        assert!(!crate::daemon::control_api::is_mutating_operation(
            operation
        ));
    }
    for operation in [
        "production.start",
        "production.resume",
        "production.cancel",
        "production.budget",
        "production.pause",
        "production.music",
    ] {
        assert!(crate::daemon::control_api::is_mutating_operation(operation));
        assert!(willdeep_runtime_protocol::SUPPORTED_OPERATIONS.contains(&operation));
    }
}

#[tokio::test]
async fn cancelled_async_receipts_get_settlement_only_grants_and_cannot_buy_or_advance() {
    use willdeep_core::plugin::{PluginHostRequests, PluginPermission, PluginRequestContext};
    let (root, mut workflow) = fixture(20);
    let old = request(&mut workflow, GenerationKind::Music);
    budget::reserve(&mut workflow, &old).unwrap();
    budget::submitted(&mut workflow, old.request_id, "fixture-existing-music").unwrap();
    let node = workflow
        .nodes
        .iter_mut()
        .find(|node| node.id == old.node_id)
        .unwrap();
    node.arguments = json!({"workflowID":workflow.id,"taskID":node.id,"grant":node.grant,
        "inputDigest":node.input_digest,"input":{"stage":"generate_music","attempt":1}});
    let arguments = node.arguments.clone();
    let grant = node.grant;
    workflow.cancel_requested = true;
    workflow.status = ProductionStatus::Cancelled;
    Store::new(&root.0)
        .update(workflow.id, |saved| {
            *saved = workflow.clone();
            Ok(())
        })
        .unwrap();
    let host = host::Host {
        home: root.0.clone(),
        workflow_id: workflow.id,
        inner: crate::plugin_host_requests::HostRequests::new(
            root.0.clone(),
            root.0.join("config.toml"),
            std::sync::Arc::new(std::sync::RwLock::new(vec![root.0.clone()])),
        ),
    };
    let context = PluginRequestContext {
        plugin_id: PLUGIN.into(),
        permissions: vec![PluginPermission::AiChat, PluginPermission::NetworkAccess],
    };
    let permission = host
        .handle(&context, host::AUTHORIZE, arguments.clone())
        .await
        .unwrap();
    assert_eq!(permission["settlementOnly"], true);
    assert_eq!(permission["receipts"].as_array().unwrap().len(), 1);
    let mut forged = arguments.clone();
    forged["grant"] = json!(Uuid::new_v4());
    assert!(
        host.handle(&context, host::AUTHORIZE, forged)
            .await
            .is_err()
    );
    assert!(
        host.handle(
            &context,
            willdeep_core::plugin::host_requests::AI_COMPLETE,
            json!({"messages":[]})
        )
        .await
        .is_err()
    );
    let mut fresh = old.clone();
    fresh.request_id = Uuid::new_v4();
    assert!(
        host.handle(
            &context,
            host::METER,
            json!({"taskID":old.node_id,"grant":grant,
        "operation":"reserve","arguments":fresh})
        )
        .await
        .is_err()
    );
    host.handle(
        &context,
        host::METER,
        json!({"taskID":old.node_id,"grant":grant,"operation":"settle",
        "arguments":{"requestID":old.request_id,"actual":{"music_calls":1,"music_seconds":12},
        "response":{"durationMs":12000},"succeeded":true}}),
    )
    .await
    .unwrap();
    let saved = Store::new(&root.0).get(workflow.id).unwrap();
    assert_eq!(saved.status, ProductionStatus::Cancelled);
    assert_eq!(saved.nodes, workflow.nodes);
    assert_eq!(saved.generations.len(), 1);
    assert_eq!(saved.generations[0].state, GenerationState::Settled);
    assert_eq!(
        budget::usage(&saved).unwrap().used[&ProductionUnit::MusicSeconds],
        12
    );
    assert!(!host::needs_reconciliation(&saved));
    assert!(
        host.handle(&context, host::AUTHORIZE, arguments)
            .await
            .is_err()
    );
}

#[test]
fn cancelled_reconciliation_requires_one_owned_node_and_preserves_unknown_requests() {
    let (_root, mut workflow) = fixture(80);
    let music = request(&mut workflow, GenerationKind::Music);
    budget::reserve(&mut workflow, &music).unwrap();
    budget::unknown(&mut workflow, music.request_id).unwrap();
    workflow.cancel_requested = true;
    workflow.status = ProductionStatus::Cancelled;
    assert_eq!(
        host::cancelled_node(&workflow).unwrap().unwrap().id,
        music.node_id
    );
    assert_eq!(
        budget::usage(&workflow).unwrap().reserved[&ProductionUnit::MusicSeconds],
        10
    );
    budget::submitted(&mut workflow, music.request_id, "recovered-existing-job").unwrap();
    assert_eq!(workflow.status, ProductionStatus::Cancelled);
    let video = request(&mut workflow, GenerationKind::Video);
    assert_ne!(music.node_id, video.node_id);
    assert!(host::cancelled_node(&workflow).is_err());
}

#[test]
fn an_unknown_synchronous_request_cannot_be_relabelled_as_a_submitted_async_job() {
    for kind in [
        GenerationKind::Model,
        GenerationKind::Image,
        GenerationKind::Tts,
    ] {
        let (_root, mut workflow) = fixture(20);
        let pending = request(&mut workflow, kind);
        budget::reserve(&mut workflow, &pending).unwrap();
        budget::unknown(&mut workflow, pending.request_id).unwrap();
        assert!(budget::submitted(&mut workflow, pending.request_id, "not-an-async-job").is_err());
        assert_eq!(workflow.generations[0].state, GenerationState::Unknown);
        assert!(workflow.generations[0].external_id.is_none());
        assert!(!budget::can_reconcile(&workflow).unwrap());
    }
}

#[test]
fn no_media_node_can_dispatch_before_whole_series_scripts_pass() {
    let (root, workflow) = fixture(80);
    let gate = workflow
        .nodes
        .iter()
        .find(|node| node.stage == ProductionStage::ReviewScripts && node.range.is_none())
        .unwrap()
        .id;
    let gate_index = workflow
        .nodes
        .iter()
        .position(|node| node.id == gate)
        .unwrap();
    Store::new(&root.0)
        .update(workflow.id, |saved| {
            for node in &mut saved.nodes[..gate_index] {
                node.state = NodeState::Completed;
            }
            Ok(())
        })
        .unwrap();
    let next = engine::prepare(&root.0, workflow.id).unwrap();
    assert_eq!(next.id, gate);
    assert_eq!(
        next.arguments["input"]["repairTargets"]
            .as_array()
            .unwrap()
            .len(),
        80
    );
    assert!(
        workflow
            .nodes
            .iter()
            .filter(|node| node.stage == ProductionStage::Storyboard)
            .all(|node| node.dependencies.contains(&gate))
    );
}

#[test]
fn outline_repair_targets_are_limited_to_the_current_batch() {
    let (root, workflow) = fixture(80);
    let index = workflow
        .nodes
        .iter()
        .position(|node| {
            node.stage == ProductionStage::ReviewOutlineBatch
                && node.range == Some(EpisodeRange { first: 9, last: 16 })
        })
        .unwrap();
    Store::new(&root.0)
        .update(workflow.id, |saved| {
            for node in &mut saved.nodes[..index] {
                node.state = NodeState::Completed;
            }
            Ok(())
        })
        .unwrap();
    let review = engine::prepare(&root.0, workflow.id).unwrap();
    let targets = review.arguments["input"]["repairTargets"]
        .as_array()
        .unwrap();
    assert_eq!(targets.len(), 1);
    assert_eq!(targets[0]["range"], json!({"first":9,"last":16}));
}

#[tokio::test]
async fn list_uses_compact_episode_summaries_without_full_node_payloads_or_provider_responses() {
    let (root, workflow) = fixture(80);
    let value = operation(&root.0, Uuid::new_v4(), "production.list", json!({}))
        .await
        .unwrap();
    assert_eq!(value[0]["id"], json!(workflow.id));
    assert_eq!(value[0]["episodes"].as_array().unwrap().len(), 80);
    assert!(value[0].get("nodes").is_none());
    assert!(value[0].get("generations").is_none());
    assert!(value.to_string().len() < 250_000);
}

#[test]
fn concurrent_reservations_cannot_overdraw_a_shared_media_quota() {
    let (root, mut workflow) = fixture(20);
    workflow
        .input
        .limits
        .insert(ProductionUnit::VideoCalls, Some(1));
    let first = request(&mut workflow, GenerationKind::Video);
    let mut second = first.clone();
    second.request_id = Uuid::new_v4();
    second.request_digest = "c".repeat(64);
    Store::new(&root.0)
        .update(workflow.id, |saved| {
            *saved = workflow.clone();
            Ok(())
        })
        .unwrap();
    let successes = std::thread::scope(|scope| {
        let handles: Vec<_> = [first, second]
            .into_iter()
            .map(|request| {
                let root = &root;
                scope.spawn(move || {
                    Store::new(&root.0)
                        .update(workflow.id, |saved| {
                            budget::reserve(saved, &request)?;
                            budget::submitted(saved, request.request_id, "fixture-live-job")
                        })
                        .is_ok()
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .filter(|result| *result)
            .count()
    });
    assert_eq!(successes, 1);
    assert_eq!(
        Store::new(&root.0)
            .get(workflow.id)
            .unwrap()
            .generations
            .len(),
        1
    );
}

#[path = "image_retry_tests.rs"]
mod image_retry_tests;

#[path = "image_recovery_tests.rs"]
mod image_recovery_tests;
