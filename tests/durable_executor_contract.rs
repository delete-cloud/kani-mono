use kani_mono::controlplane::{DurableRunCoordinator, DurableSessionService};
use kani_mono::executor::LocalDaemonExecutor;
use kani_mono::runtime::ReplayRuntime;
use kani_mono::session::{RunStatus, RunTarget};
use kani_mono::storage::SqliteControlPlaneStore;
use kani_mono::stream::{DisplayEventKind, RuntimeEventKind};
use pretty_assertions::assert_eq;
use tempfile::tempdir;

#[test]
fn durable_run_coordinator_persists_runtime_and_display_events() {
    let dir = tempdir().expect("temp dir");
    let db_path = dir.path().join("controlplane.sqlite3");

    let (run_id, final_result) = {
        let store = SqliteControlPlaneStore::open(&db_path).expect("store opens");
        let mut service = DurableSessionService::new(store);
        let session = service
            .create_session(RunTarget::local_daemon("/workspace"))
            .expect("session persists");
        let executor = LocalDaemonExecutor::new(ReplayRuntime::new("durable answer"));
        let mut coordinator = DurableRunCoordinator::new(executor);

        let run = coordinator
            .start_run(&mut service, &session.session_id, "persisted input")
            .expect("run completes");

        assert_eq!(run.status, RunStatus::Completed);
        (run.run_id, run.result.expect("completed run has result"))
    };

    let reopened = SqliteControlPlaneStore::open(&db_path).expect("store reopens");
    assert_eq!(final_result, "durable answer");
    assert_eq!(
        reopened
            .replay_runtime_events(&run_id, 0)
            .expect("runtime replay succeeds")
            .iter()
            .map(|event| event.kind.clone())
            .collect::<Vec<_>>(),
        vec![
            RuntimeEventKind::RunStarted,
            RuntimeEventKind::ModelDelta,
            RuntimeEventKind::RunCompleted,
        ]
    );
    assert_eq!(
        reopened
            .replay_display_events(&run_id, 0)
            .expect("display replay succeeds")
            .iter()
            .map(|event| event.kind.clone())
            .collect::<Vec<_>>(),
        vec![
            DisplayEventKind::Progress,
            DisplayEventKind::AssistantTextDelta,
            DisplayEventKind::FinalResult,
        ]
    );
}
