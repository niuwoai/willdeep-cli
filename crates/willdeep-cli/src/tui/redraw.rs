use super::*;

/// 一帧必须连同 EndSynchronizedUpdate 一起 flush，终端才会在随后的 RPC 前显示它。
/// 即使绘制失败也结束同步更新，避免终端继续扣住后续输出。
pub(super) fn redraw<B: ratatui::backend::Backend + io::Write>(
    term: &mut Terminal<B>,
    app: &mut App,
    skills: &SkillCatalog,
    session: &Session,
) -> Result<()> {
    queue!(term.backend_mut(), terminal::BeginSynchronizedUpdate)?;
    let drawn = draw(term, app, skills, session);
    execute!(term.backend_mut(), terminal::EndSynchronizedUpdate)?;
    drawn
}
