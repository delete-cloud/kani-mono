use kani_mono::daemon::LocalDaemonProcess;
use kani_mono::runtime::ReplayRuntime;
use kani_mono::session::RunTarget;
use kani_mono::stream::DisplayEventKind;
use pretty_assertions::assert_eq;
use tempfile::tempdir;

#[test]
fn local_daemon_process_survives_client_drop_until_explicit_stop() {
    let dir = tempdir().expect("temp dir");
    let db_path = dir.path().join("process.sqlite3");

    let process = LocalDaemonProcess::start(&db_path, ReplayRuntime::new("process answer"))
        .expect("daemon process starts");
    assert!(process.is_running());

    let session_id = {
        let mut client = process.client();
        let session = client
            .create_session(RunTarget::local_daemon("/workspace"))
            .expect("session persists through process");
        session.session_id
    };

    assert!(
        process.is_running(),
        "dropping a client must not stop daemon"
    );

    let mut client = process.client();
    let run = client
        .start_run(&session_id, "process input")
        .expect("run completes through process");
    let display_events = client
        .replay_display_events(&run.run_id, 0)
        .expect("display replay succeeds through process");

    assert_eq!(run.result.as_deref(), Some("process answer"));
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

    process.stop().expect("daemon process stops");
    assert!(!process.is_running());
}
