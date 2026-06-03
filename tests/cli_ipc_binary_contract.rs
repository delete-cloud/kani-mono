#[cfg(unix)]
use std::process::Command;

#[cfg(unix)]
use kani_mono::daemon::LocalDaemonIpcServer;
#[cfg(unix)]
use kani_mono::runtime::ReplayRuntime;
#[cfg(unix)]
use pretty_assertions::assert_eq;
#[cfg(unix)]
use tempfile::tempdir;

#[cfg(unix)]
#[test]
fn binary_cli_can_use_socket_backed_daemon_client_path() {
    let dir = tempdir().expect("temp dir");
    let db_path = dir.path().join("cli-ipc.sqlite3");
    let socket_path = dir.path().join("daemon.sock");
    let socket_arg = socket_path.to_str().expect("socket path is utf-8");

    let server =
        LocalDaemonIpcServer::start(&socket_path, &db_path, ReplayRuntime::new("socket answer"))
            .expect("ipc server starts");

    let create = run_binary([
        "session",
        "create",
        "--socket",
        socket_arg,
        "--workspace",
        "/workspace",
    ]);
    let session_id = value_for_key(&create, "session_id");

    let run = run_binary([
        "run",
        "start",
        "--socket",
        socket_arg,
        "--session",
        session_id,
        "--input",
        "hello through socket",
    ]);
    let run_id = value_for_key(&run, "run_id");
    assert_eq!(value_for_key(&run, "result"), "socket answer");

    let replay = run_binary([
        "display", "replay", "--socket", socket_arg, "--run", run_id, "--after", "0",
    ]);
    assert_eq!(
        replay.lines().collect::<Vec<_>>(),
        vec![
            "1 progress",
            "2 assistant_text_delta text=socket answer",
            "3 final_result result=socket answer",
        ]
    );

    server.stop().expect("ipc server stops");
}

#[cfg(unix)]
fn run_binary<const N: usize>(args: [&str; N]) -> String {
    let output = Command::new(binary_path())
        .args(args)
        .output()
        .expect("binary runs");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("stdout is utf-8")
}

#[cfg(unix)]
fn binary_path() -> String {
    std::env::var("CARGO_BIN_EXE_kani-mono").expect("cargo provides binary path")
}

#[cfg(unix)]
fn value_for_key<'a>(output: &'a str, key: &str) -> &'a str {
    output
        .lines()
        .find_map(|line| line.strip_prefix(&format!("{key}=")))
        .unwrap_or_else(|| panic!("missing {key}=... in output: {output}"))
}
