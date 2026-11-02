//! 跨 run 复用的上下文压缩结果。
//!
//! 自动压缩只在请求里换掉投影、不改写 durable 历史（见 [`crate::agent`] 的
//! `request_messages_accounted`），所以同一段历史每来一条新消息都要重新摘要
//! 一遍。历史长到上百万 token 时，这一步按窗口切块、逐块串行、再合并，一次要
//! 跑七八分钟，而且每轮都跑。
//!
//! 把摘要连同它覆盖的前缀存进会话文件，下一次 run 先验指纹再复用：历史没动的
//! 前缀不再重摘，只有新长出来的尾部走增量摘要。
//!
//! 这只是一份缓存。校验不过就当它不存在——最坏是多花一次摘要的钱，绝不会把
//! 别的历史的摘要喂给模型。

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextSummary {
    /// 覆盖到 durable 消息列表的第几条（不含）。
    ///
    /// 用 durable 下标而不是分页投影的下标：投影会随窗口大小改写工具输出、
    /// 随重复的 host instruction 去重，下标不稳定；durable 历史只增不改。
    pub through: usize,
    /// `messages[..through]`（跳过 system）的 SHA-256，十六进制。
    pub fingerprint: String,
    /// 生成这份摘要时 system 提示（系统提示 + 全局/项目规则 + 技能目录）的
    /// SHA-256。规则一改，旧摘要里可能还留着已经删掉的指令，必须作废。
    #[serde(default)]
    pub system_fingerprint: String,
    /// 摘要正文（模型原样输出，`compact_prefix` 自己加外壳）。
    pub summary: String,
}
