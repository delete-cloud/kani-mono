use kani_mono::controlplane::{DurableSessionService, RunCoordinator};
use kani_mono::executor::LocalDaemonExecutor;
use kani_mono::runtime::ReplayRuntime;
use kani_mono::session::{RunStatus, RunTarget};
use kani_mono::storage::SqliteControlPlaneStore;
use kani_mono::stream::EventLog;
use pretty_assertions::assert_eq;
use tempfile::tempdir;

#[test]
fn run_coordinator_uses_store_backed_controlplane_service() {
    let dir = tempdir().expect("temp dir");
    let db_path = dir.path().join("controlplane.sqlite3");

    let (session_id, run_id) = {
        let store = SqliteControlPlaneStore::open(&db_path).expect("store opens");
        let mut service = DurableSessionService::new(store);
        let session = service
            .create_session(RunTarget::local_daemon("/workspace"))
            .expect("session persists");
        let executor = LocalDaemonExecutor::new(ReplayRuntime::new("durable answer"));
        let mut coordinator = RunCoordinator::new(executor);
        let mut events = EventLog::default();

        let run = coordinator
            .start_run(
                &mut service,
                &mut events,
                &session.session_id,
                "persisted input",
            )
            .expect("run completes");

        assert_eq!(run.status, RunStatus::Completed);
        assert_eq!(run.result.as_deref(), Some("durable answer"));
        (session.session_id, run.run_id)
    };

    let reopened = SqliteControlPlaneStore::open(&db_path).expect("store reopens");
    let loaded_session = reopened
        .load_session(&session_id)
        .expect("session load succeeds")
        .expect("session exists");
    let loaded_run = reopened
        .load_run(&run_id)
        .expect("run load succeeds")
        .expect("run exists");

    assert_eq!(loaded_session.current_run_id, Some(run_id.clone()));
    assert_eq!(loaded_run.status, RunStatus::Completed);
    assert_eq!(loaded_run.result.as_deref(), Some("durable answer"));
    assert_eq!(loaded_run.input, "persisted input");
}
