use super::*;

pub(super) fn initial_nodes(spec: &SeriesSpec) -> Vec<ProductionNode> {
    let preflight = node(ProductionStage::Preflight, None, vec![]);
    let plan = node(ProductionStage::SeriesPlan, None, vec![preflight.id]);
    let plan_review = node(ProductionStage::ReviewSeriesPlan, None, vec![plan.id]);
    let mut previous = plan_review.id;
    let mut nodes = vec![preflight, plan, plan_review];
    for range in spec.episode_ranges() {
        let mut outline = node(ProductionStage::OutlineBatch, None, vec![previous]);
        outline.range = Some(range);
        let mut review = node(ProductionStage::ReviewOutlineBatch, None, vec![outline.id]);
        review.range = Some(range);
        previous = review.id;
        nodes.extend([outline, review]);
    }
    let mut scripts = Vec::new();
    for episode in 1..=spec.episode_count {
        let write = node(ProductionStage::WriteEpisode, Some(episode), vec![previous]);
        let review = node(
            ProductionStage::ReviewEpisode,
            Some(episode),
            vec![write.id],
        );
        previous = review.id;
        scripts.push(review.id);
        nodes.extend([write, review]);
    }
    let script_reviews = ranged_reviews(&mut nodes, spec, &scripts, ProductionStage::ReviewScripts);
    let scripts_gate = node(ProductionStage::ReviewScripts, None, script_reviews);
    let gate_id = scripts_gate.id;
    nodes.push(scripts_gate);
    let mut films = Vec::new();
    for (index, script) in scripts.into_iter().enumerate() {
        let episode = index as u16 + 1;
        let episode_nodes = media_nodes(episode, script, gate_id);
        films.push(
            episode_nodes
                .last()
                .expect("media graph always includes film review")
                .id,
        );
        nodes.extend(episode_nodes);
    }
    let film_reviews = ranged_reviews(&mut nodes, spec, &films, ProductionStage::ReviewSeries);
    let continuity = node(ProductionStage::ReviewSeries, None, film_reviews);
    let delivery = node(ProductionStage::Deliver, None, vec![continuity.id]);
    nodes.extend([continuity, delivery]);
    nodes
}

fn ranged_reviews(
    nodes: &mut Vec<ProductionNode>,
    spec: &SeriesSpec,
    sources: &[Uuid],
    stage: ProductionStage,
) -> Vec<Uuid> {
    spec.episode_ranges()
        .into_iter()
        .map(|range| {
            let mut review = node(
                stage,
                None,
                sources[usize::from(range.first - 1)..usize::from(range.last)].to_vec(),
            );
            review.range = Some(range);
            let id = review.id;
            nodes.push(review);
            id
        })
        .collect()
}

fn media_nodes(episode: u16, script: Uuid, scripts_gate: Uuid) -> Vec<ProductionNode> {
    use ProductionStage::*;
    let board = node(Storyboard, Some(episode), vec![script, scripts_gate]);
    let board_review = node(ReviewStoryboard, Some(episode), vec![board.id]);
    let assets = node(DesignAssets, Some(episode), vec![board_review.id]);
    let generate_assets = node(GenerateAssets, Some(episode), vec![assets.id]);
    let asset_review = node(ReviewAssets, Some(episode), vec![generate_assets.id]);
    let frames = node(GenerateFrames, Some(episode), vec![asset_review.id]);
    let frame_review = node(ReviewFrames, Some(episode), vec![frames.id]);
    let dialogue = node(GenerateDialogue, Some(episode), vec![assets.id]);
    let dialogue_review = node(ReviewDialogue, Some(episode), vec![dialogue.id]);
    let music = node(GenerateMusic, Some(episode), vec![board_review.id]);
    let music_review = node(ReviewMusic, Some(episode), vec![music.id]);
    let video = node(
        GenerateVideo,
        Some(episode),
        vec![frame_review.id, dialogue_review.id],
    );
    let video_review = node(ReviewVideo, Some(episode), vec![video.id]);
    let compose = node(
        ComposeEpisode,
        Some(episode),
        vec![video_review.id, music_review.id],
    );
    let film_review = node(ReviewFilm, Some(episode), vec![compose.id]);
    vec![
        board,
        board_review,
        assets,
        generate_assets,
        asset_review,
        frames,
        frame_review,
        dialogue,
        dialogue_review,
        music,
        music_review,
        video,
        video_review,
        compose,
        film_review,
    ]
}

fn node(stage: ProductionStage, episode: Option<u16>, dependencies: Vec<Uuid>) -> ProductionNode {
    ProductionNode {
        id: Uuid::new_v4(),
        stage,
        episode,
        range: None,
        subject: None,
        dependencies,
        input_digest: String::new(),
        grant: Uuid::nil(),
        state: NodeState::Pending,
        attempt: 0,
        history: vec![],
        arguments: json!({}),
        result: None,
    }
}
