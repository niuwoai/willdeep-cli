//! `events.ndjson` 的稀疏字节偏移索引。
//!
//! 事件日志只追加，读取却一直是从文件头逐行解析：日志涨到 168 MB / 90 万条后，
//! 一次 `read_after` 要在写线程里扫完整份文件，控制面、健康检查和 TUI 事件流都
//! 被它拖住。这里按固定步长记录「序号 → 字节偏移」采样点，读取时二分定位到最近
//! 的采样点再向前扫，代价从 O(整份日志) 降到 O(采样步长 + limit)。
use anyhow::{Context, Result};
use std::fs::File;
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::Path;

use super::RuntimeEvent;

/// 每多少个事件记录一个采样点。256 条约 48 KB，整份日志只有几千个采样点。
const INDEX_STRIDE: u64 = 256;

#[derive(Debug, Default)]
pub(super) struct EventIndex {
    /// `(起始序号, 字节偏移)`，按序号升序。
    samples: Vec<(u64, u64)>,
    /// 已写入的最后一个序号。
    head: u64,
}

impl EventIndex {
    /// 全量扫描一次已有日志，建立索引；返回最后一条事件的序号（空日志为 0）。
    ///
    /// 启动时本来就要读到最后一条来确定序号，这里顺手只保留采样点，不再把整份
    /// 事件都留在内存里。
    pub(super) fn scan(path: &Path) -> Result<Self> {
        let mut index = Self::default();
        if !path.exists() {
            return Ok(index);
        }
        let mut reader = BufReader::new(File::open(path)?);
        let mut offset = 0u64;
        let mut line = String::new();
        loop {
            line.clear();
            let read = reader.read_line(&mut line)?;
            if read == 0 {
                break;
            }
            if !line.trim().is_empty() {
                let event: RuntimeEvent = serde_json::from_str(&line)
                    .with_context(|| format!("parse Runtime event from {}", path.display()))?;
                index.observe(event.sequence, offset);
            }
            offset += read as u64;
        }
        Ok(index)
    }

    pub(super) fn head(&self) -> u64 {
        self.head
    }

    /// 记录一条事件在文件里的起始偏移。
    pub(super) fn observe(&mut self, sequence: u64, offset: u64) {
        // 序号本该连续；出现断档就再加一个采样点，避免按 `sequence % stride`
        // 采样时整段没有落点。
        let needs_sample = self.samples.is_empty()
            || sequence.is_multiple_of(INDEX_STRIDE)
            || self
                .samples
                .last()
                .is_some_and(|(last, _)| sequence != last + 1);
        if needs_sample {
            self.samples.push((sequence, offset));
        }
        self.head = self.head.max(sequence);
    }

    /// 读取 `after` 之后最多 `limit` 条事件。
    pub(super) fn read_after(
        &self,
        path: &Path,
        after: u64,
        limit: usize,
    ) -> Result<Vec<RuntimeEvent>> {
        if limit == 0 || !path.exists() {
            return Ok(Vec::new());
        }
        // 序号单调递增：游标已经到最新一条时，后面不可能还有更小的序号。
        if after >= self.head {
            return Ok(Vec::new());
        }
        let mut file = File::open(path)?;
        file.seek(SeekFrom::Start(self.offset_for(after)))?;
        let mut events = Vec::new();
        for line in BufReader::new(file).lines() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            let event: RuntimeEvent = serde_json::from_str(&line)
                .with_context(|| format!("parse Runtime event from {}", path.display()))?;
            if event.sequence > after {
                events.push(event);
                if events.len() >= limit {
                    break;
                }
            }
        }
        Ok(events)
    }

    /// 不大于 `after` 的最后一个采样点偏移；没有采样点就从文件头开始。
    fn offset_for(&self, after: u64) -> u64 {
        let position = self
            .samples
            .partition_point(|(sequence, _)| *sequence <= after);
        position
            .checked_sub(1)
            .map_or(0, |index| self.samples[index].1)
    }
}
