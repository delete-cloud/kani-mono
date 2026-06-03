#[cfg(unix)]
use std::io::{BufRead, BufReader};
#[cfg(unix)]
use std::process::{Child, Command, Stdio};
#[cfg(unix)]
use std::time::{Duration, Instant};

#[cfg(unix)]
use pretty_assertions::assert_eq;
#[cfg(unix)]
use tempfile::tempdir;

#[cfg(unix)]
#[test]
fn binary_daemon_status_reports_running_socket_daemon() {
    let dir = tempdir().expect("temp dir");
    let db_path = dir.path().join("status.sqlite3");
    let socket_path = dir.path().join("status.sock");
    let db_arg = db_path.to_str().expect("db path is utf-8");
    let socket_arg = socket_path.to_str().expect("socket path is utf-8");

    let mut child = Command::new(binary_path())
        .args(["daemon", "serve", "--socket", socket_arg, "--store", db_arg])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("daemon serve starts");
    let mut stdout = BufReader::new(child.stdout.take().expect("stdout is piped"));
    let mut ready = String::new();
    stdout
        .read_line(&mut ready)
        .expect("ready line is readable");
    assert_eq!(
        ready.trim(),
        format!("daemon_ready socket={socket_arg} store={db_arg}")
    );

    let status = run_binary(["daemon", "status", "--socket", socket_arg]);
    assert_eq!(
        status.lines().collect::<Vec<_>>(),
        vec!["daemon_status=running", &format!("socket={socket_arg}"),]
    );

    drop(child.stdin.take());
    let mut stopped = String::new();
    stdout
        .read_line(&mut stopped)
        .expect("stopped line is readable");
    assert_eq!(stopped.trim(), "daemon_stopped");
    wait_for_exit(&mut child);
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
fn wait_for_exit(child: &mut Child) {
    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline {
        if child
            .try_wait()
            .expect("child status is readable")
            .is_some()
        {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let _ = child.kill();
    panic!("daemon serve did not exit after stdin closed");
}

#[cfg(unix)]
fn binary_path() -> String {
    std::env::var("CARGO_BIN_EXE_kani-mono").expect("cargo provides binary path")
}
