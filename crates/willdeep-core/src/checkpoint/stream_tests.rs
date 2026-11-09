use super::*;
use std::sync::Mutex;

#[derive(Default)]
struct Saved(Mutex<Vec<RunCheckpoint>>);

impl CheckpointSink for Saved {
    fn save(&self, checkpoint: &RunCheckpoint) -> Result<(), String> {
        self.0.lock().unwrap().push(checkpoint.clone());
        Ok(())
    }
}

#[test]
fn streamed_message_keeps_the_model_that_produced_it() {
    let sink = Saved::default();
    let mut recorder = CheckpointRecorder::new(Some(&sink));
    recorder.record(&[Message::user("work")], 1, 0, 0).unwrap();
    recorder
        .record_stream(Some("第一段"), Some("glm-5"), 1, 0, 0)
        .unwrap();
    // 后续增量不再带模型：章在第一次落盘时盖上，追加正文不能把它冲掉，
    // 也不能换成别的。
    recorder
        .record_stream(Some("，第二段"), None, 1, 0, 0)
        .unwrap();
    recorder.flush_stream().unwrap();
    let saved = sink.0.lock().unwrap();
    let message = saved.last().unwrap().messages.last().unwrap();
    assert_eq!(message.content, "第一段，第二段");
    assert_eq!(message.model.as_deref(), Some("glm-5"));

    // 说不上来模型的流什么都不盖，不拿上一条的模型顶上。
    drop(saved);
    let sink = Saved::default();
    let mut recorder = CheckpointRecorder::new(Some(&sink));
    recorder.record(&[Message::user("work")], 1, 0, 0).unwrap();
    recorder
        .record_stream(Some("no model"), None, 1, 0, 0)
        .unwrap();
    recorder.flush_stream().unwrap();
    let saved = sink.0.lock().unwrap();
    assert_eq!(saved.last().unwrap().messages.last().unwrap().model, None);
}

#[test]
fn text_bursts_do_not_rewrite_large_history_for_every_delta() {
    let sink = Saved::default();
    let mut recorder = CheckpointRecorder::new(Some(&sink));
    recorder
        .record(&[Message::user("history".repeat(100_000))], 1, 0, 0)
        .unwrap();
    for _ in 0..1_000 {
        recorder.record_stream(Some("文"), None, 1, 0, 0).unwrap();
    }
    assert_eq!(
        sink.0.lock().unwrap().len(),
        2,
        "boundary plus first text only"
    );
    recorder.flush_stream().unwrap();
    let saved = sink.0.lock().unwrap();
    assert_eq!(saved.len(), 3);
    assert_eq!(
        saved.last().unwrap().messages.last().unwrap().content,
        "文".repeat(1_000)
    );
}

#[test]
fn byte_limit_and_usage_force_a_durable_stream_boundary() {
    let sink = Saved::default();
    let mut recorder = CheckpointRecorder::new(Some(&sink));
    recorder.record(&[Message::user("work")], 1, 0, 0).unwrap();
    recorder
        .record_stream(Some("first"), None, 1, 0, 0)
        .unwrap();
    recorder
        .record_stream(Some(&"x".repeat(STREAM_CHECKPOINT_BYTES)), None, 1, 0, 0)
        .unwrap();
    assert_eq!(sink.0.lock().unwrap().len(), 3);
    recorder.record_stream(Some("tail"), None, 1, 0, 0).unwrap();
    recorder.record_stream(None, None, 1, 4, 5).unwrap();
    let saved = sink.0.lock().unwrap();
    assert_eq!(saved.len(), 4);
    let last = saved.last().unwrap();
    assert!(last.messages.last().unwrap().content.ends_with("tail"));
    assert_eq!(last.metadata.output_tokens, 5);
}

#[test]
fn dropping_an_unfinished_recorder_flushes_pending_text() {
    let sink = Saved::default();
    {
        let mut recorder = CheckpointRecorder::new(Some(&sink));
        recorder.record(&[Message::user("work")], 1, 0, 0).unwrap();
        recorder
            .record_stream(Some("first"), None, 1, 0, 0)
            .unwrap();
        recorder
            .record_stream(Some(" tail"), None, 1, 0, 0)
            .unwrap();
        assert_eq!(sink.0.lock().unwrap().len(), 2);
    }
    let saved = sink.0.lock().unwrap();
    assert_eq!(saved.len(), 3);
    assert_eq!(
        saved.last().unwrap().messages.last().unwrap().content,
        "first tail"
    );
    assert_eq!(
        saved.last().unwrap().metadata.status,
        CheckpointStatus::Running
    );
}
