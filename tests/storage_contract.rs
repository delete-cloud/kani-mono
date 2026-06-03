use kani_mono::controlplane::SessionService;
use kani_mono::session::{RunStatus, RunTarget};
use kani_mono::storage::SqliteControlPlaneStore;
use kani_mono::stream::{RuntimeEvent, RuntimeEventKind};
use pretty_assertions::assert_eq;
use serde_json::json;
use tempfile::tempdir;

#[test]
fn sqlite_store_persists_controlplane_records_across_reopen() {
    let dir = tempdir().expect("temp dir");
    let db_path = dir.path().join("controlplane.sqlite3");
    let mut service = SessionService::new();
    let session = service.create_session(RunTarget::local_daemon("/workspace"));
    let run = service
        .start_run(&session.session_id, "persist run")
        .expect("run starts");
    let approval = service
        .request_approval(&run.run_id, "tool-call-1", json!({"cmd": "cargo test"}))
        .expect("approval is requested");
    let approval = service
        .resolve_approval(&approval.approval_id, true, "approved once")
        .expect("approval is resolved");
    let cancel = service
        .request_cancel(&run.run_id, "user requested")
        .expect("cancel is requested");
    let run = service.run(&run.run_id).expect("run is durable in service");

    {
        let store = SqliteControlPlaneStore::open(&db_path).expect("store opens");
        store.save_session(&session).expect("session persists");
        store.save_run(run).expect("run persists");
        store.save_approval(&approval).expect("approval persists");
        store.save_cancel(&cancel).expect("cancel persists");
    }

    let reopened = SqliteControlPlaneStore::open(&db_path).expect("store reopens");
    let loaded_session = reopened
        .load_session(&session.session_id)
        .expect("session load succeeds")
        .expect("session exists");
    let loaded_run = reopened
        .load_run(&run.run_id)
        .expect("run load succeeds")
        .expect("run exists");
    let loaded_approval = reopened
        .load_approval(&approval.approval_id)
        .expect("approval load succeeds")
        .expect("approval exists");
    let loaded_cancel = reopened
        .load_cancel(&cancel.cancel_id)
        .expect("cancel load succeeds")
        .expect("cancel exists");

    assert_eq!(loaded_session.session_id, session.session_id);
    assert_eq!(
        loaded_session.default_run_target,
        session.default_run_target
    );
    assert_eq!(loaded_session.tape_id, session.tape_id);
    assert_eq!(loaded_run.status, RunStatus::Cancelling);
    assert_eq!(loaded_run.target, RunTarget::local_daemon("/workspace"));
    assert_eq!(loaded_approval.decision.as_deref(), Some("approved"));
    assert_eq!(loaded_approval.feedback.as_deref(), Some("approved once"));
    assert_eq!(loaded_cancel.reason, "user requested");
}

#[test]
fn sqlite_runtime_events_are_idempotent_and_replay_by_sequence() {
    let dir = tempdir().expect("temp dir");
    let db_path = dir.path().join("events.sqlite3");
    let store = SqliteControlPlaneStore::open(&db_path).expect("store opens");

    let first = store
        .append_runtime_event(RuntimeEvent::new(
            "event-1",
            "run-1",
            RuntimeEventKind::ModelDelta,
            json!({"text": "first"}),
        ))
        .expect("first event persists");
    let duplicate = store
        .append_runtime_event(RuntimeEvent::new(
            "event-1",
            "run-1",
            RuntimeEventKind::ModelDelta,
            json!({"text": "must not overwrite"}),
        ))
        .expect("duplicate event returns existing row");
    let second = store
        .append_runtime_event(RuntimeEvent::new(
            "event-2",
            "run-1",
            RuntimeEventKind::RunCompleted,
            json!({"status": "completed"}),
        ))
        .expect("second event persists");

    assert_eq!(first.sequence, 1);
    assert_eq!(duplicate.sequence, 1);
    assert_eq!(duplicate.payload, json!({"text": "first"}));
    assert!(second.sequence > duplicate.sequence);

    let replay = store
        .replay_runtime_events("run-1", 0)
        .expect("replay succeeds");
    assert_eq!(
        replay
            .iter()
            .map(|event| event.event_id.as_str())
            .collect::<Vec<_>>(),
        vec!["event-1", "event-2"]
    );
    assert_eq!(
        store
            .replay_runtime_events("run-1", 1)
            .expect("cursor replay succeeds")
            .iter()
            .map(|event| event.event_id.as_str())
            .collect::<Vec<_>>(),
        vec!["event-2"]
    );
}
