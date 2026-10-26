//! Durable series production contracts. Media quantities never masquerade as text tokens.
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, path::PathBuf};
use uuid::Uuid;

pub const MAX_EPISODES: u16 = 80;
pub const DEFAULT_EPISODES: u16 = 40;
pub const OUTLINE_BATCH_SIZE: u16 = 8;
pub const SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SeriesSpec {
    pub episode_count: u16,
    pub episode_seconds: u16,
    pub aspect: Aspect,
}

impl Default for SeriesSpec {
    fn default() -> Self {
        Self {
            episode_count: DEFAULT_EPISODES,
            episode_seconds: 90,
            aspect: Aspect::Portrait,
        }
    }
}

impl SeriesSpec {
    pub fn validate(&self) -> Result<(), &'static str> {
        if !(1..=MAX_EPISODES).contains(&self.episode_count) {
            return Err("production_episode_count_out_of_range");
        }
        if !(30..=300).contains(&self.episode_seconds) {
            return Err("production_episode_duration_out_of_range");
        }
        Ok(())
    }

    /// Bounded outline and read-through batches, with exact, gap-free coverage.
    pub fn episode_ranges(&self) -> Vec<EpisodeRange> {
        (1..=self.episode_count)
            .step_by(usize::from(OUTLINE_BATCH_SIZE))
            .map(|first| EpisodeRange {
                first,
                last: (first + OUTLINE_BATCH_SIZE - 1).min(self.episode_count),
            })
            .collect()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Aspect {
    Portrait,
    Landscape,
    Square,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EpisodeRange {
    pub first: u16,
    pub last: u16,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProductionUnit {
    ModelCalls,
    Tokens,
    Images,
    TtsCharacters,
    MusicCalls,
    MusicSeconds,
    VideoCalls,
    VideoSeconds,
}

pub type Quantities = BTreeMap<ProductionUnit, u64>;
/// Explicit null authorizes unlimited media; missing entries never authorize spending.
pub type ResourceLimits = BTreeMap<ProductionUnit, Option<u64>>;

pub fn validate_limits(limits: &ResourceLimits) -> Result<(), &'static str> {
    use ProductionUnit::*;
    let units = [
        ModelCalls,
        Tokens,
        Images,
        TtsCharacters,
        MusicCalls,
        MusicSeconds,
        VideoCalls,
        VideoSeconds,
    ];
    if limits.len() != units.len() || units.iter().any(|unit| !limits.contains_key(unit)) {
        return Err("production_media_budget_required");
    }
    if limits.values().any(|limit| *limit == Some(0))
        || [ModelCalls, Tokens]
            .iter()
            .any(|unit| limits[unit].is_none())
    {
        return Err("production_media_budget_required");
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GenerationKind {
    Model,
    Image,
    Tts,
    Music,
    Video,
}

impl GenerationKind {
    pub fn units(self) -> &'static [ProductionUnit] {
        use ProductionUnit::*;
        match self {
            Self::Model => &[ModelCalls, Tokens],
            Self::Image => &[Images],
            Self::Tts => &[TtsCharacters],
            Self::Music => &[MusicCalls, MusicSeconds],
            Self::Video => &[VideoCalls, VideoSeconds],
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProductionStart {
    pub brief: Option<String>,
    pub source_screenplay_id: Option<Uuid>,
    pub workspace: PathBuf,
    pub spec: SeriesSpec,
    /// Explicit cumulative quantities, including reviews and quality retries. No implied media grant.
    pub limits: ResourceLimits,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_model_calls_per_episode: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin_session_id: Option<Uuid>,
}

impl ProductionStart {
    pub fn validate(&self) -> Result<(), &'static str> {
        self.spec.validate()?;
        match (&self.brief, self.source_screenplay_id) {
            (Some(brief), None) if !brief.trim().is_empty() && brief.chars().count() <= 4_000 => {}
            (None, Some(_)) => {}
            _ => return Err("production_requires_brief_or_source"),
        }
        if !self.workspace.is_absolute() {
            return Err("production_absolute_workspace_required");
        }
        if self.max_model_calls_per_episode == Some(0) {
            return Err("production_episode_call_limit_invalid");
        }
        validate_limits(&self.limits)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProductionStatus {
    Running,
    WaitingDependency,
    WaitingUser,
    BudgetLimited,
    Completed,
    Cancelled,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProductionStage {
    Preflight,
    SeriesPlan,
    ReviewSeriesPlan,
    OutlineBatch,
    ReviewOutlineBatch,
    WriteEpisode,
    ReviewEpisode,
    ReviewScripts,
    ReviseEpisode,
    Storyboard,
    ReviewStoryboard,
    DesignAssets,
    GenerateAssets,
    ReviewAssets,
    GenerateFrames,
    ReviewFrames,
    GenerateDialogue,
    ReviewDialogue,
    GenerateMusic,
    ReviewMusic,
    GenerateVideo,
    ReviewVideo,
    RemediateVideo,
    ComposeEpisode,
    ReviewFilm,
    ReviewSeries,
    Deliver,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeState {
    Pending,
    Running,
    WaitingExternal,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProductionNode {
    pub id: Uuid,
    pub stage: ProductionStage,
    pub episode: Option<u16>,
    pub range: Option<EpisodeRange>,
    pub subject: Option<String>,
    pub dependencies: Vec<Uuid>,
    pub input_digest: String,
    pub grant: Uuid,
    pub state: NodeState,
    pub attempt: u16,
    pub history: Vec<serde_json::Value>,
    pub arguments: serde_json::Value,
    pub result: Option<serde_json::Value>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GenerationState {
    Reserved,
    Submitted,
    Settled,
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GenerationReceipt {
    pub request_id: Uuid,
    pub node_id: Uuid,
    pub kind: GenerationKind,
    pub request_digest: String,
    pub state: GenerationState,
    pub reservation: Quantities,
    pub actual: Option<Quantities>,
    pub provider: String,
    pub external_id: Option<String>,
    pub evidence_digest: Option<String>,
    pub succeeded: Option<bool>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProductionBudgetChange {
    pub request_id: Uuid,
    pub at: u64,
    pub previous: ResourceLimits,
    pub limits: ResourceLimits,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProductionWorkflow {
    pub schema: u32,
    pub id: Uuid,
    pub input: ProductionStart,
    pub status: ProductionStatus,
    pub created_at: u64,
    pub updated_at: u64,
    pub cancel_requested: bool,
    #[serde(default)]
    pub pause_requested: bool,
    #[serde(default)]
    pub music_deferred: bool,
    #[serde(default)]
    pub control_history: Vec<serde_json::Value>,
    pub nodes: Vec<ProductionNode>,
    pub generations: Vec<GenerationReceipt>,
    pub budget_changes: Vec<ProductionBudgetChange>,
    pub domain: Option<serde_json::Value>,
    pub error: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_series_have_exact_bounded_review_coverage() {
        for count in [1, 3, 20, 24, 40, 79, 80] {
            let spec = SeriesSpec {
                episode_count: count,
                ..SeriesSpec::default()
            };
            spec.validate().unwrap();
            let ranges = spec.episode_ranges();
            let coverage: Vec<_> = ranges.iter().flat_map(|r| r.first..=r.last).collect();
            assert_eq!(coverage, (1..=count).collect::<Vec<_>>());
            assert!(ranges.iter().all(|r| r.last - r.first < OUTLINE_BATCH_SIZE));
        }
    }

    #[test]
    fn invalid_series_cannot_silently_fall_back_to_three_episodes() {
        for count in [0, 81, u16::MAX] {
            assert!(
                SeriesSpec {
                    episode_count: count,
                    ..SeriesSpec::default()
                }
                .validate()
                .is_err()
            );
        }
        assert_eq!(SeriesSpec::default().episode_count, 40);
    }

    #[test]
    fn text_budget_does_not_authorize_media_or_ambiguous_source() {
        let mut input = ProductionStart {
            brief: Some("县城父女亲情".into()),
            source_screenplay_id: None,
            max_model_calls_per_episode: None,
            origin_session_id: None,
            workspace: std::env::temp_dir().join("production-protocol-fixture"),
            spec: SeriesSpec::default(),
            limits: [
                (ProductionUnit::ModelCalls, 1_000),
                (ProductionUnit::Tokens, 5_000_000),
            ]
            .into_iter()
            .map(|(unit, limit)| (unit, Some(limit)))
            .collect(),
        };
        assert_eq!(input.validate(), Err("production_media_budget_required"));
        for kind in [
            GenerationKind::Image,
            GenerationKind::Tts,
            GenerationKind::Music,
            GenerationKind::Video,
        ] {
            for unit in kind.units() {
                input.limits.insert(*unit, Some(100));
            }
        }
        input.validate().unwrap();
        input.source_screenplay_id = Some(Uuid::new_v4());
        assert_eq!(input.validate(), Err("production_requires_brief_or_source"));
        input.brief = None;
        input.validate().unwrap();
    }

    #[test]
    fn unlimited_is_explicit_media_only_and_old_finite_archives_remain_readable() {
        let values = serde_json::json!({"model_calls":24000,"tokens":100000000,"images":null,"tts_characters":null,"music_calls":null,"music_seconds":null,"video_calls":null,"video_seconds":null});
        let mut limits: ResourceLimits = serde_json::from_value(values).unwrap();
        validate_limits(&limits).unwrap();
        limits.insert(ProductionUnit::Tokens, None);
        assert!(validate_limits(&limits).is_err());
        limits.insert(ProductionUnit::Tokens, Some(100_000_000));
        limits.remove(&ProductionUnit::Images);
        assert!(validate_limits(&limits).is_err());
        limits.insert(ProductionUnit::Images, Some(0));
        assert!(validate_limits(&limits).is_err());
        limits.insert(ProductionUnit::Images, Some(1));
        validate_limits(&limits).unwrap();
    }
}
