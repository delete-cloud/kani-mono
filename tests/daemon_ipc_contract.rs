#[cfg(unix)]
use kani_mono::daemon::{LocalDaemonIpcClient, LocalDaemonIpcServer};
#[cfg(unix)]
use kani_mono::runtime::ReplayRuntime;
#[cfg(unix)]
use kani_mono::session::RunTarget;
#[cfg(unix)]
use kani_mono::stream::DisplayEventKind;
#[cfg(unix)]
use pretty_assertions::assert_eq;
#[cfg(unix)]
use tempfile::tempdir;

#[cfg(unix)]
#[test]
fn unix_socket_daemon_serves_multiple_clients_without_losing_state() {
    let dir = tempdir().expect("temp dir");
    let db_path = dir.path().join("ipc.sqlite3");
    let socket_path = dir.path().join("daemon.sock");

    let server =
        LocalDaemonIpcServer::start(&socket_path, &db_path, ReplayRuntime::new("ipc answer"))
            .expect("ipc server starts");

    let session_id = {
        let mut client = LocalDaemonIpcClient::connect(&socket_path).expect("client connects");
        let session = client
            .create_session(RunTarget::local_daemon("/workspace"))
            .expect("session is created over ipc");
        session.session_id
    };

    let mut client = LocalDaemonIpcClient::connect(&socket_path).expect("second client connects");
    let run = client
        .start_run(&session_id, "ipc input")
        .expect("run completes over ipc");
    let display_events = client
        .replay_display_events(&run.run_id, 0)
        .expect("display replay works over ipc");

    assert_eq!(run.result.as_deref(), Some("ipc answer"));
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

    server.stop().expect("ipc server stops");
    assert!(!socket_path.exists());
}
