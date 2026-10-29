use super::*;
use std::path::PathBuf;

/// 读会话不该付出执行指纹的代价：指纹要序列化整段对话，而轮询、列表、
/// 归属校验这些读路径从不写回。
#[test]
fn loading_a_session_does_not_fingerprint_its_execution_state() {
    let (root, store, session) = fixture();
    let loaded = store.load(session.id).unwrap();
    let baseline = loaded
        .metadata_baseline
        .clone()
        .expect("加载必须带基线，否则下一次保存会被拒绝");
    assert!(!baseline.has_execution_fingerprint(), "加载不该算执行指纹");

    // 基线缺指纹即「这份快照没动过执行状态」：并发写来的新结果仍被保留。
    store
        .update(session.id, |session| {
            session
                .messages
                .push(Message::assistant("written while stale", Vec::new()));
        })
        .unwrap();
    let mut stale = loaded;
    stale.model = Some("renamed model".into());
    store.save(&mut stale).unwrap();

    let saved = store.load(session.id).unwrap();
    assert_eq!(saved.model.as_deref(), Some("renamed model"));
    assert_eq!(
        saved.messages.last().unwrap().content,
        "written while stale",
        "加载时没算指纹，也不能因此把并发写入的执行结果盖掉"
    );
    std::fs::remove_dir_all(root).unwrap();
}

/// `update` 返回的快照带着完整基线：调用方拿去改消息再保存时，
/// 冲突检测要比对的是改动前的执行状态，不是改完之后的。
#[test]
fn a_snapshot_from_update_keeps_its_execution_baseline_across_a_later_save() {
    let (root, store, session) = fixture();
    let mut snapshot = store
        .update(session.id, |session| {
            session.model = Some("first model".into());
        })
        .unwrap();
    assert!(
        snapshot
            .metadata_baseline
            .as_ref()
            .expect("update 返回的快照必须带基线")
            .has_execution_fingerprint(),
        "update 会把快照交还给调用方，基线必须已经算好执行指纹"
    );

    // 并发写者追加了一条消息：两边都动了执行状态，保存必须拒绝，磁盘保持原样。
    store
        .update(session.id, |session| {
            session
                .messages
                .push(Message::assistant("concurrent result", Vec::new()));
        })
        .unwrap();
    snapshot
        .messages
        .push(Message::assistant("my own result", Vec::new()));
    assert!(
        matches!(
            store.save(&mut snapshot),
            Err(SessionError::ConcurrentUpdate("execution"))
        ),
        "双方都改了执行状态时必须拒绝，不能让后写的那方盖掉并发结果"
    );

    let saved = store.load(session.id).unwrap();
    let contents: Vec<_> = saved
        .messages
        .iter()
        .map(|message| message.content.as_str())
        .collect();
    assert!(
        contents.contains(&"concurrent result"),
        "被拒绝的保存不能改动磁盘，实际消息：{contents:?}"
    );
    assert!(
        !contents.contains(&"my own result"),
        "冲突的那条消息不该落盘"
    );
    std::fs::remove_dir_all(root).unwrap();
}

fn fixture() -> (PathBuf, SessionStore, Session) {
    let root = std::env::temp_dir().join(format!("session-read-baseline-{}", Uuid::new_v4()));
    let store = SessionStore::new(&root);
    let mut session = Session::new(root.clone(), None, "initial");
    session.messages.push(Message::user("hello"));
    store.save(&mut session).unwrap();
    (root, store, session)
}
