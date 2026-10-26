use super::*;

pub(super) fn control(
    workflow: &mut ProductionWorkflow,
    operation: &str,
    deferred: Option<bool>,
    request: Uuid,
    at: u64,
) -> Result<()> {
    anyhow::ensure!(
        !workflow.cancel_requested
            && !matches!(
                workflow.status,
                ProductionStatus::Completed | ProductionStatus::Cancelled
            ),
        "production_control_terminal"
    );
    match operation {
        "pause" => {
            anyhow::ensure!(
                workflow.status == ProductionStatus::Running,
                "production_pause_not_running"
            );
            workflow.pause_requested = true;
        }
        "music" => {
            let choice = deferred.context("production_music_choice_required")?;
            anyhow::ensure!(
                !choice
                    || !workflow.nodes.iter().any(|node| matches!(
                        node.stage,
                        ProductionStage::GenerateMusic | ProductionStage::ReviewMusic
                    ) && matches!(
                        node.state,
                        NodeState::Running | NodeState::WaitingExternal
                    )),
                "production_music_current_stage_busy"
            );
            workflow.music_deferred = choice;
        }
        _ => anyhow::bail!("production_scheduling_operation_unknown"),
    }
    workflow
        .control_history
        .push(json!({"requestID":request,"at":at,"operation":operation,
        "musicDeferred":workflow.music_deferred,"pauseRequested":workflow.pause_requested}));
    Ok(())
}

pub(super) fn pause_if_ready(workflow: &mut ProductionWorkflow) -> bool {
    if workflow.pause_requested
        && !workflow.cancel_requested
        && workflow.generations.iter().all(|receipt| {
            receipt.state == GenerationState::Settled
                || recovery::retained_unknown(workflow, receipt)
        })
    {
        workflow.status = ProductionStatus::WaitingUser;
        workflow.error = Some("production_paused".into());
        return true;
    }
    false
}

pub(super) fn deferred(workflow: &ProductionWorkflow, stage: ProductionStage) -> bool {
    workflow.music_deferred
        && matches!(
            stage,
            ProductionStage::GenerateMusic | ProductionStage::ReviewMusic
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> (PathBuf, ProductionWorkflow) {
        let home = std::env::temp_dir().join(format!("production-music-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&home).unwrap();
        let input = ProductionStart {
            brief: Some("test".into()),
            source_screenplay_id: None,
            origin_session_id: None,
            max_model_calls_per_episode: None,
            workspace: home.clone(),
            spec: SeriesSpec {
                episode_count: 80,
                ..SeriesSpec::default()
            },
            limits: [
                ProductionUnit::ModelCalls,
                ProductionUnit::Tokens,
                ProductionUnit::Images,
                ProductionUnit::TtsCharacters,
                ProductionUnit::MusicCalls,
                ProductionUnit::MusicSeconds,
                ProductionUnit::VideoCalls,
                ProductionUnit::VideoSeconds,
            ]
            .into_iter()
            .map(|unit| (unit, Some(1_000_000)))
            .collect(),
        };
        let workflow = Store::new(&home).create(Uuid::new_v4(), input).unwrap();
        (home, workflow)
    }
    #[test]
    fn music_deferral_selects_video_then_next_episode_without_changing_music_proofs_or_budget() {
        let (home, mut workflow) = fixture();
        let first_music = workflow
            .nodes
            .iter()
            .position(|node| node.stage == ProductionStage::GenerateMusic)
            .unwrap();
        for node in &mut workflow.nodes[..first_music] {
            node.state = NodeState::Completed;
        }
        let input = workflow.input.clone();
        control(&mut workflow, "music", Some(true), Uuid::new_v4(), 1).unwrap();
        Store::new(&home)
            .update(workflow.id, |current| {
                *current = workflow.clone();
                Ok(())
            })
            .unwrap();
        let next = engine::prepare(&home, workflow.id).unwrap();
        assert_eq!(next.stage, ProductionStage::GenerateVideo);
        workflow = Store::new(&home).get(workflow.id).unwrap();
        assert_eq!(workflow.nodes[first_music].state, NodeState::Pending);
        for node in &mut workflow.nodes {
            if node.episode == Some(1)
                && matches!(
                    node.stage,
                    ProductionStage::GenerateVideo | ProductionStage::ReviewVideo
                )
            {
                node.state = NodeState::Completed;
            }
        }
        Store::new(&home)
            .update(workflow.id, |current| {
                *current = workflow.clone();
                Ok(())
            })
            .unwrap();
        let next = engine::prepare(&home, workflow.id).unwrap();
        assert_eq!(next.stage, ProductionStage::Storyboard);
        assert_eq!(next.episode, Some(2));
        assert_eq!(workflow.input, input);
        assert!(workflow.budget_changes.is_empty());
        std::fs::remove_dir_all(home).unwrap();
    }
    #[test]
    fn deferred_music_blocks_final_delivery_and_reinclusion_does_not_resume() {
        let (home, mut workflow) = fixture();
        for node in &mut workflow.nodes {
            if !matches!(
                node.stage,
                ProductionStage::GenerateMusic
                    | ProductionStage::ReviewMusic
                    | ProductionStage::ComposeEpisode
                    | ProductionStage::ReviewFilm
                    | ProductionStage::ReviewSeries
                    | ProductionStage::Deliver
            ) {
                node.state = NodeState::Completed;
            }
        }
        control(&mut workflow, "music", Some(true), Uuid::new_v4(), 1).unwrap();
        Store::new(&home)
            .update(workflow.id, |current| {
                *current = workflow.clone();
                Ok(())
            })
            .unwrap();
        assert!(
            engine::prepare(&home, workflow.id)
                .unwrap_err()
                .to_string()
                .contains("production_music_deferred")
        );
        workflow.status = ProductionStatus::WaitingDependency;
        control(&mut workflow, "music", Some(false), Uuid::new_v4(), 2).unwrap();
        assert_eq!(workflow.status, ProductionStatus::WaitingDependency);
        assert!(!workflow.music_deferred);
        assert_eq!(workflow.control_history.len(), 2);
        std::fs::remove_dir_all(home).unwrap();
    }
    #[test]
    fn safe_pause_waits_for_receipts_and_never_cancels_or_settles_unknown_usage() {
        let (home, mut workflow) = fixture();
        control(&mut workflow, "pause", None, Uuid::new_v4(), 1).unwrap();
        assert!(!workflow.cancel_requested);
        let node_id = workflow
            .nodes
            .iter()
            .find(|node| node.stage == ProductionStage::WriteEpisode)
            .unwrap()
            .id;
        workflow.generations.push(GenerationReceipt {
            request_id: Uuid::new_v4(),
            node_id,
            kind: GenerationKind::Model,
            request_digest: "a".repeat(64),
            state: GenerationState::Reserved,
            reservation: [
                (ProductionUnit::ModelCalls, 1),
                (ProductionUnit::Tokens, 10),
            ]
            .into(),
            actual: None,
            provider: "fixture".into(),
            external_id: None,
            evidence_digest: None,
            succeeded: None,
        });
        assert!(!pause_if_ready(&mut workflow));
        assert_eq!(workflow.status, ProductionStatus::Running);
        workflow.generations[0].state = GenerationState::Unknown;
        let unknown = workflow.generations[0].clone();
        assert!(!pause_if_ready(&mut workflow));
        assert_eq!(workflow.generations[0], unknown);
        workflow.generations[0].state = GenerationState::Settled;
        workflow.generations[0].actual =
            Some([(ProductionUnit::ModelCalls, 1), (ProductionUnit::Tokens, 5)].into());
        assert!(pause_if_ready(&mut workflow));
        assert_eq!(workflow.status, ProductionStatus::WaitingUser);
        assert_eq!(workflow.error.as_deref(), Some("production_paused"));
        assert_eq!(
            workflow.generations[0].actual.as_ref().unwrap()[&ProductionUnit::Tokens],
            5
        );
        workflow.status = ProductionStatus::Cancelled;
        let before = workflow.clone();
        assert!(control(&mut workflow, "music", Some(true), Uuid::new_v4(), 2).is_err());
        assert_eq!(workflow, before);
        std::fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn deferral_does_not_revoke_an_active_music_stage_or_its_provider_job() {
        let (home, mut workflow) = fixture();
        let node = workflow
            .nodes
            .iter_mut()
            .find(|node| node.stage == ProductionStage::GenerateMusic)
            .unwrap();
        node.state = NodeState::WaitingExternal;
        let before = workflow.clone();
        assert!(control(&mut workflow, "music", Some(true), Uuid::new_v4(), 1).is_err());
        assert_eq!(workflow, before);
        std::fs::remove_dir_all(home).unwrap();
    }
}
