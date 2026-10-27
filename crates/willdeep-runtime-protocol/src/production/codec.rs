use super::ProductionWorkflow;

// Concrete entry points keep these large Serde instantiations in the protocol
// crate's optimization boundary, even when the CLI uses an unoptimized dev build.
#[inline(never)]
pub fn decode_workflow(bytes: &[u8]) -> Result<ProductionWorkflow, serde_json::Error> {
    serde_json::from_slice(bytes)
}

#[inline(never)]
pub fn encode_workflow(workflow: &ProductionWorkflow) -> Result<Vec<u8>, serde_json::Error> {
    serde_json::to_vec_pretty(workflow)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::production::*;
    use serde_json::json;
    use uuid::Uuid;

    fn fixture() -> ProductionWorkflow {
        ProductionWorkflow {
            schema: SCHEMA_VERSION,
            id: Uuid::nil(),
            input: ProductionStart {
                brief: Some("照相馆里的父女".into()),
                source_screenplay_id: None,
                workspace: "/fixture".into(),
                spec: SeriesSpec::default(),
                limits: [
                    (ProductionUnit::Tokens, Some(100)),
                    (ProductionUnit::Images, None),
                ]
                .into_iter()
                .collect(),
                max_model_calls_per_episode: Some(300),
                origin_session_id: None,
            },
            status: ProductionStatus::WaitingUser,
            created_at: 1,
            updated_at: 2,
            cancel_requested: false,
            pause_requested: true,
            music_deferred: true,
            control_history: vec![json!({"未知预留":"保留\n不核销","capacity":u64::MAX})],
            nodes: vec![ProductionNode {
                id: Uuid::nil(),
                stage: ProductionStage::Storyboard,
                episode: Some(1),
                range: None,
                subject: None,
                dependencies: vec![],
                input_digest: "a".repeat(64),
                grant: Uuid::nil(),
                state: NodeState::WaitingExternal,
                attempt: 2,
                history: vec![json!({"原稿":"对白与修订记录","nested":[null,true,1.25]})],
                arguments: json!({"text":"原文\n引号\"与反斜杠\\"}),
                result: None,
            }],
            generations: vec![GenerationReceipt {
                request_id: Uuid::nil(),
                node_id: Uuid::nil(),
                kind: GenerationKind::Model,
                request_digest: "b".repeat(64),
                state: GenerationState::Unknown,
                reservation: [(ProductionUnit::Tokens, 70)].into_iter().collect(),
                actual: None,
                provider: "fixture".into(),
                external_id: None,
                evidence_digest: None,
                succeeded: None,
            }],
            budget_changes: vec![],
            domain: Some(json!({"historicalHold":{"tokens":20}})),
            error: Some("production_paused".into()),
        }
    }

    #[test]
    fn complete_archive_bytes_and_unknown_usage_are_identical() {
        let workflow = fixture();
        let existing = serde_json::to_vec_pretty(&workflow).unwrap();
        assert_eq!(encode_workflow(&workflow).unwrap(), existing);
        let decoded = decode_workflow(&existing).unwrap();
        assert_eq!(decoded, workflow);
        assert_eq!(encode_workflow(&decoded).unwrap(), existing);
    }

    #[test]
    fn malformed_unknown_fields_and_trailing_input_keep_strict_serde_errors() {
        let valid = encode_workflow(&fixture()).unwrap();
        let mut unknown: serde_json::Value = serde_json::from_slice(&valid).unwrap();
        unknown["unexpected"] = json!(true);
        let mut trailing = valid.clone();
        trailing.extend_from_slice(b" invalid");
        for invalid in [
            b"{".to_vec(),
            serde_json::to_vec(&unknown).unwrap(),
            trailing,
        ] {
            let old = serde_json::from_slice::<ProductionWorkflow>(&invalid).unwrap_err();
            let new = decode_workflow(&invalid).unwrap_err();
            assert_eq!(new.to_string(), old.to_string());
        }
    }

    #[test]
    fn legacy_optional_defaults_preserve_the_existing_normalized_encoding() {
        let mut value = serde_json::to_value(fixture()).unwrap();
        for field in ["pause_requested", "music_deferred", "control_history"] {
            value.as_object_mut().unwrap().remove(field);
        }
        let bytes = serde_json::to_vec(&value).unwrap();
        let old: ProductionWorkflow = serde_json::from_slice(&bytes).unwrap();
        let new = decode_workflow(&bytes).unwrap();
        assert_eq!(new, old);
        assert_eq!(
            encode_workflow(&new).unwrap(),
            serde_json::to_vec_pretty(&old).unwrap()
        );
    }
}
