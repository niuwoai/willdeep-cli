use super::*;
use std::fs::File;
use std::io::{BufRead, BufReader};

/// 参照实现：从文件头逐行解析。索引读取的结果必须与它逐条一致。
fn full_scan(path: &Path, after: u64, limit: usize) -> Vec<RuntimeEvent> {
    let file = File::open(path).unwrap();
    let mut events = Vec::new();
    for line in BufReader::new(file).lines() {
        let line = line.unwrap();
        if line.trim().is_empty() {
            continue;
        }
        let event: RuntimeEvent = serde_json::from_str(&line).unwrap();
        if event.sequence > after {
            events.push(event);
            if events.len() >= limit {
                break;
            }
        }
    }
    events
}

fn on_disk(path: &Path, after: u64, limit: usize) -> Vec<RuntimeEvent> {
    let index = EventIndex::scan(path).unwrap();
    index.read_after(path, after, limit).unwrap()
}

fn preview(index: usize) -> String {
    format!(
        "task_id=fixture {}",
        serde_json::json!({"type":"assistant_text_delta","text":format!("chunk-{index}")})
    )
}

#[test]
fn preview_batches_are_durable_ordered_and_flushed_before_terminal_events() {
    let root = std::env::temp_dir().join(format!("wd-group-commit-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join("events.ndjson");
    let log = EventLog::open_for_tests(path.clone()).unwrap();
    let mut receiver = log.subscribe();
    for index in 0..200 {
        assert_eq!(
            log.append("task.output", preview(index)).unwrap().sequence,
            index as u64 + 1
        );
    }
    log.append("turn.completed", "done").unwrap();
    assert_eq!(log.latest_sequence(), 201);
    assert!(
        log.writer.commits() < 50,
        "preview deltas must share disk syncs"
    );
    let durable = log.read_after(0, 1000).unwrap();
    assert_eq!(durable.len(), 201);
    for (index, expected) in durable.iter().enumerate() {
        assert_eq!(expected.sequence, index as u64 + 1);
        assert_eq!(receiver.try_recv().unwrap().sequence, expected.sequence);
    }
    drop(log);
    let reopened = EventLog::open_for_tests(path).unwrap();
    assert_eq!(reopened.latest_sequence(), 201);
    assert_eq!(reopened.append("next", "resumed").unwrap().sequence, 202);
    drop(reopened);
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn a_single_preview_is_delivered_without_waiting_for_another_token() {
    let root = std::env::temp_dir().join(format!("wd-group-first-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join("events.ndjson");
    let log = EventLog::open_for_tests(path.clone()).unwrap();
    let mut receiver = log.subscribe();
    log.append("task.output", preview(0)).unwrap();
    let event = tokio::time::timeout(Duration::from_secs(2), receiver.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(event.sequence, 1);
    assert_eq!(on_disk(&path, 0, 10).len(), 1);
    drop(log);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn drop_and_read_barriers_flush_tail_previews() {
    let root = std::env::temp_dir().join(format!("wd-group-tail-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join("events.ndjson");
    let log = EventLog::open_for_tests(path.clone()).unwrap();
    log.append("task.output", preview(0)).unwrap();
    assert_eq!(log.read_after(0, 10).unwrap().len(), 1);
    log.append("task.output", preview(1)).unwrap();
    drop(log);
    assert_eq!(on_disk(&path, 0, 10).len(), 2);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn failed_disk_write_never_broadcasts_or_advances_committed_cursor() {
    let root = std::env::temp_dir().join(format!("wd-group-failure-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    let log = EventLog::open_for_tests(root.join("events.ndjson")).unwrap();
    let mut receiver = log.subscribe();
    std::fs::remove_dir_all(root).unwrap();
    log.append("task.output", preview(0)).unwrap();
    assert!(log.append("turn.completed", "done").is_err());
    assert_eq!(log.latest_sequence(), 0);
    assert!(receiver.try_recv().is_err());
    assert!(log.read_after(0, 10).is_err());
}

#[tokio::test]
async fn contiguous_live_stream_never_rescans_the_journal() {
    let root = std::env::temp_dir().join(format!("wd-live-fast-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    let log = Arc::new(EventLog::open_for_tests(root.join("events.ndjson")).unwrap());
    let receiver = log.subscribe();
    for index in 0..200 {
        log.append("task.output", preview(index)).unwrap();
    }
    log.append("turn.completed", "done").unwrap();
    let (_shutdown, shutdown) = tokio::sync::watch::channel(false);
    let mut state = LiveEventState {
        backlog: VecDeque::new(),
        receiver,
        shutdown,
        log: log.clone(),
        cursor: 0,
    };
    for sequence in 1..=201 {
        assert_eq!(next_event(&mut state).await.unwrap().sequence, sequence);
    }
    assert_eq!(log.writer.reads(), 0);
    drop(state);
    drop(log);
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn a_live_gap_backfills_history_before_returning_the_new_event() {
    let root = std::env::temp_dir().join(format!("wd-live-gap-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    let log = Arc::new(EventLog::open_for_tests(root.join("events.ndjson")).unwrap());
    for index in 0..3 {
        log.append("history", index.to_string()).unwrap();
    }
    let receiver = log.subscribe();
    log.append("new", "live").unwrap();
    let (_shutdown, shutdown) = tokio::sync::watch::channel(false);
    let mut state = LiveEventState {
        backlog: VecDeque::new(),
        receiver,
        shutdown,
        log: log.clone(),
        cursor: 0,
    };
    for sequence in 1..=4 {
        assert_eq!(next_event(&mut state).await.unwrap().sequence, sequence);
    }
    assert_eq!(log.writer.reads(), 1);
    drop(state);
    drop(log);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn index_reads_match_a_full_scan_across_sample_boundaries() {
    let root = std::env::temp_dir().join(format!("wd-index-boundary-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join("events.ndjson");
    let log = EventLog::open_for_tests(path.clone()).unwrap();
    for index in 0..700 {
        log.append("task.output", preview(index)).unwrap();
    }
    drop(log);

    let reopened = EventLog::open_for_tests(path.clone()).unwrap();
    for after in [0, 1, 255, 256, 257, 511, 512, 513, 698, 699, 700, 701] {
        for limit in [1, 7, 256, 1_000] {
            let indexed = reopened.read_after(after, limit).unwrap();
            let expected = full_scan(&path, after, limit);
            assert_eq!(
                indexed
                    .iter()
                    .map(|event| event.sequence)
                    .collect::<Vec<_>>(),
                expected
                    .iter()
                    .map(|event| event.sequence)
                    .collect::<Vec<_>>(),
                "after={after} limit={limit}"
            );
        }
    }
    drop(reopened);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn appended_events_stay_indexed_after_a_restart() {
    let root = std::env::temp_dir().join(format!("wd-index-restart-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join("events.ndjson");
    let log = EventLog::open_for_tests(path.clone()).unwrap();
    for index in 0..300 {
        log.append("task.output", preview(index)).unwrap();
    }
    drop(log);

    let reopened = EventLog::open_for_tests(path.clone()).unwrap();
    assert_eq!(reopened.append("next", "resumed").unwrap().sequence, 301);
    for index in 300..620 {
        reopened.append("task.output", preview(index)).unwrap();
    }
    for after in [0, 299, 300, 301, 557, 619, 620, 621] {
        let indexed = reopened.read_after(after, 500).unwrap();
        let expected = full_scan(&path, after, 500);
        assert_eq!(
            indexed
                .iter()
                .map(|event| event.sequence)
                .collect::<Vec<_>>(),
            expected
                .iter()
                .map(|event| event.sequence)
                .collect::<Vec<_>>(),
            "after={after}"
        );
    }
    drop(reopened);
    std::fs::remove_dir_all(root).unwrap();
}
