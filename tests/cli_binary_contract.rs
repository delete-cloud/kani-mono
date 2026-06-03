use std::process::Command;

use pretty_assertions::assert_eq;
use tempfile::tempdir;

#[test]
fn binary_cli_uses_local_daemon_client_path() {
    let dir = tempdir().expect("temp dir");
    let db_path = dir.path().join("binary-cli.sqlite3");
    let db_arg = db_path.to_str().expect("temp path is utf-8");

    let create = run_binary([
        "session",
        "create",
        "--store",
        db_arg,
        "--workspace",
        "/workspace",
    ]);
    let session_id = value_for_key(&create, "session_id");

    let run = Command::new(binary_path())
        .args([
            "run",
            "start",
            "--store",
            db_arg,
            "--session",
            session_id,
            "--input",
            "hello from binary",
        ])
        .env("KANI_MONO_REPLAY_RESPONSE", "binary answer")
        .output()
        .expect("binary runs");
    assert!(
        run.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&run.stderr)
    );
    let run_stdout = String::from_utf8(run.stdout).expect("stdout is utf-8");
    let run_id = value_for_key(&run_stdout, "run_id");
    assert_eq!(value_for_key(&run_stdout, "result"), "binary answer");

    let replay = run_binary([
        "display", "replay", "--store", db_arg, "--run", run_id, "--after", "0",
    ]);
    assert_eq!(
        replay.lines().collect::<Vec<_>>(),
        vec![
            "1 progress",
            "2 assistant_text_delta text=binary answer",
            "3 final_result result=binary answer",
        ]
    );
}

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

fn binary_path() -> String {
    std::env::var("CARGO_BIN_EXE_kani-mono").expect("cargo provides binary path")
}

fn value_for_key<'a>(output: &'a str, key: &str) -> &'a str {
    output
        .lines()
        .find_map(|line| line.strip_prefix(&format!("{key}=")))
        .unwrap_or_else(|| panic!("missing {key}=... in output: {output}"))
}
