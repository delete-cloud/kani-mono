use kani_mono::daemon::LocalDaemon;
use kani_mono::runtime::ReplayRuntime;
use kani_mono::session::RunTarget;
use kani_mono::stream::DisplayEventKind;
use pretty_assertions::assert_eq;
use tempfile::tempdir;

#[test]
fn local_daemon_creates_session_runs_task_and_replays_display_events() {
    let dir = tempdir().expect("temp dir");
    let db_path = dir.path().join("daemon.sqlite3");

    let (session_id, run_id) = {
        let runtime = ReplayRuntime::new("daemon answer");
        let mut daemon = LocalDaemon::open(&db_path, runtime).expect("daemon opens");
        let session = daemon
            .create_session(RunTarget::local_daemon("/workspace"))
            .expect("session persists");
        let run = daemon
            .start_run(&session.session_id, "daemon input")
            .expect("run completes");
        let display_events = daemon
            .replay_display_events(&run.run_id, 0)
            .expect("display replay succeeds");

        assert_eq!(run.result.as_deref(), Some("daemon answer"));
        assert_eq!(
            display_events
                .iter()
                .map(|event| event.kind.clone())
                .collect::<Vec<_>>(),
            vec![
                DisplayEventKind::Progress,
                DisplayEventKind::AssistantTextDelta,
                DisplayEventKind::FinalResult,
            ]
        );
        (session.session_id, run.run_id)
    };

    let runtime = ReplayRuntime::new("unused after reopen");
    let daemon = LocalDaemon::open(&db_path, runtime).expect("daemon reopens");
    let session = daemon
        .session(&session_id)
        .expect("session load succeeds")
        .expect("session exists");
    let run = daemon
        .run(&run_id)
        .expect("run load succeeds")
        .expect("run exists");

    assert_eq!(session.current_run_id, Some(run_id.clone()));
    assert_eq!(run.result.as_deref(), Some("daemon answer"));
    assert_eq!(
        daemon
            .replay_display_events(&run_id, 0)
            .expect("display replay after reopen succeeds")
            .len(),
        3
    );
}
