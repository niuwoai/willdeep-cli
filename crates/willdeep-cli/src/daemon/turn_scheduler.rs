//! Coalesce pending scheduling hints before touching the persistent stores.
//!
//! Reconciliation can repeatedly signal the same capacity-blocked session. These
//! hints are not individual Turns: claiming consults the durable queue, and task
//! completion signals again. Retain FIFO order between distinct sessions while
//! preventing a backlog of old hints from delaying a newly queued parent Turn.
use std::collections::HashSet;
use tokio::sync::mpsc::UnboundedReceiver;
use uuid::Uuid;

pub(super) fn ready_batch(first: Uuid, receiver: &mut UnboundedReceiver<Uuid>) -> Vec<Uuid> {
    let mut seen = HashSet::from([first]);
    let mut ready = vec![first];
    // Snapshot the length so producers cannot keep this drain running forever.
    // Hints arriving during or after this batch remain eligible for the next one.
    let pending = receiver.len();
    for _ in 0..pending {
        let Ok(session) = receiver.try_recv() else {
            break;
        };
        if seen.insert(session) {
            ready.push(session);
        }
    }
    ready
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::mpsc::unbounded_channel;

    #[test]
    fn a_capacity_backlog_does_not_hide_a_new_parent_turn() {
        const BACKLOG_SIZE: usize = 10_000;
        let (sender, mut receiver) = unbounded_channel();
        let first = Uuid::new_v4();
        let second = Uuid::new_v4();
        let parent = Uuid::new_v4();
        for _ in 0..BACKLOG_SIZE {
            sender.send(first).unwrap();
            sender.send(second).unwrap();
        }
        sender.send(parent).unwrap();
        let head = receiver.try_recv().unwrap();
        assert_eq!(
            ready_batch(head, &mut receiver),
            vec![first, second, parent]
        );
        assert!(receiver.is_empty());
    }

    #[test]
    fn distinct_sessions_keep_fifo_order_and_later_signals_are_not_suppressed() {
        let (sender, mut receiver) = unbounded_channel();
        let first = Uuid::new_v4();
        let second = Uuid::new_v4();
        let third = Uuid::new_v4();
        for session in [first, second, first, third, second] {
            sender.send(session).unwrap();
        }
        let head = receiver.try_recv().unwrap();
        assert_eq!(ready_batch(head, &mut receiver), vec![first, second, third]);
        // A completion or a new user Turn may requeue a session after claiming.
        sender.send(first).unwrap();
        let head = receiver.try_recv().unwrap();
        assert_eq!(ready_batch(head, &mut receiver), vec![first]);
    }

    #[test]
    fn closing_the_channel_keeps_every_already_queued_session() {
        let (sender, mut receiver) = unbounded_channel();
        let first = Uuid::new_v4();
        let second = Uuid::new_v4();
        for session in [first, first, second] {
            sender.send(session).unwrap();
        }
        drop(sender);
        let head = receiver.try_recv().unwrap();
        assert_eq!(ready_batch(head, &mut receiver), vec![first, second]);
        assert!(receiver.try_recv().is_err());
    }
}
