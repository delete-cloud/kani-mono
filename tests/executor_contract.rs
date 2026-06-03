use kani_mono::controlplane::{RunCoordinator, SessionService};
use kani_mono::executor::LocalDaemonExecutor;
use kani_mono::runtime::ReplayRuntime;
use kani_mono::session::{RunStatus, RunTarget};
use kani_mono::stream::{DisplayEventKind, EventLog, RuntimeEventKind};
use pretty_assertions::assert_eq;

#[test]
fn run_coordinator_schedules_local_daemon_executor_and_records_events() {
    let mut service = SessionService::new();
    let session = service.create_session(RunTarget::local_daemon("/workspace"));
    let runtime = ReplayRuntime::new("runtime completed");
    let executor = LocalDaemonExecutor::new(runtime);
    let mut coordinator = RunCoordinator::new(executor);
    let mut events = EventLog::new();

    let run = coordinator
        .start_run(&mut service, &mut events, &session.session_id, "do work")
        .expect("coordinator starts and executes run");

    assert_eq!(run.status, RunStatus::Completed);
    assert_eq!(
        service.run(&run.run_id).map(|run| &run.status),
        Some(&RunStatus::Completed)
    );
    assert_eq!(
        events
            .replay_runtime(&run.run_id, 0)
            .iter()
            .map(|event| &event.kind)
            .collect::<Vec<_>>(),
        vec![
            &RuntimeEventKind::RunStarted,
            &RuntimeEventKind::ModelDelta,
            &RuntimeEventKind::RunCompleted,
        ]
    );
    assert_eq!(
        events
            .replay_display(&run.run_id, 0)
            .iter()
            .map(|event| &event.kind)
            .collect::<Vec<_>>(),
        vec![
            &DisplayEventKind::Progress,
            &DisplayEventKind::AssistantTextDelta,
            &DisplayEventKind::FinalResult,
        ]
    );
}
