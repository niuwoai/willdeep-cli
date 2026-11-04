use crate::{Message, Session, SessionStore};
use uuid::Uuid;

#[test]
fn delegation_cap_and_parent_survive_stale_saves_and_forks() {
    let root = std::env::temp_dir().join(format!("delegation-metadata-{}", Uuid::new_v4()));
    let store = SessionStore::new(&root);
    let mut session = Session::new(root.clone(), None, "child");
    store.save(&mut session).unwrap();
    let mut stale = store.load_baselined(session.id).unwrap();
    let parent = Uuid::new_v4();
    let history_source = Uuid::new_v4();
    store
        .update(session.id, |current| {
            current.delegation_read_only = true;
            current.parent_session_id = Some(parent);
            current.started_by = Some(history_source);
        })
        .unwrap();
    stale.parent_session_id = Some(Uuid::new_v4());
    stale
        .messages
        .push(Message::assistant("completed read", Vec::new()));
    store.save(&mut stale).unwrap();
    let current = store.load(session.id).unwrap();
    assert!(current.delegation_read_only);
    assert_eq!(current.parent_session_id, Some(parent));
    assert_eq!(current.started_by, Some(history_source));
    assert_eq!(current.messages.last().unwrap().content, "completed read");
    let digest = store
        .digests()
        .into_iter()
        .find(|d| d.id == session.id)
        .unwrap();
    assert!(digest.delegation_read_only);
    assert_eq!(digest.parent_session_id, Some(parent));
    let fork = current.fork_snapshot();
    assert!(
        fork.delegation_read_only,
        "fork must not drop a persisted capability ceiling"
    );
    assert_eq!(fork.parent_session_id, Some(parent));
    assert_eq!(fork.started_by, None);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn legacy_sessions_have_no_implicit_delegation() {
    let session: Session = serde_json::from_value(serde_json::json!({
        "version":1, "id":Uuid::new_v4(), "title":"legacy", "workspace":".",
        "created_at":1, "updated_at":2, "messages":[]
    }))
    .unwrap();
    assert_eq!(session.parent_session_id, None);
    assert!(!session.delegation_read_only);
}
