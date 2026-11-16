use super::{ASSISTANT_ROW_PREFIX, App};

impl App {
    /// 新落进记录的这一行该盖哪个模型的章。
    ///
    /// 只有助手正文行才盖这一轮事件带来的产出模型。用户输入、工具行、系统行
    /// 和分隔线都留空；不知道回复模型时也不拿当前会话模型代替。
    pub(super) fn model_for_row(&self, row: &str) -> Option<String> {
        let source = if row.starts_with(ASSISTANT_ROW_PREFIX) {
            self.reply_model.as_deref()
        } else {
            None
        };
        source
            .map(str::trim)
            .filter(|model| !model.is_empty())
            .map(str::to_owned)
    }
}
