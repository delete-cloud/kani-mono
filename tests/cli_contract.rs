use kani_mono::cli::run_cli;
use kani_mono::runtime::ReplayRuntime;
use pretty_assertions::assert_eq;
use tempfile::tempdir;

#[test]
fn cli_client_creates_session_starts_run_and_replays_display_events_through_local_daemon() {
    let dir = tempdir().expect("temp dir");
    let db_path = dir.path().join("cli.sqlite3");
    let db_arg = db_path.to_str().expect("temp path is utf-8");

    let create = run_cli(
        [
            "session",
            "create",
            "--store",
            db_arg,
            "--workspace",
            "/workspace",
        ],
        ReplayRuntime::new("unused for create"),
    )
    .expect("session create succeeds");
    let session_id = value_for_key(&create, "session_id");

    let run = run_cli(
        [
            "run",
            "start",
            "--store",
            db_arg,
            "--session",
            session_id,
            "--input",
            "hello from cli",
        ],
        ReplayRuntime::new("cli answer"),
    )
    .expect("run start succeeds");
    let run_id = value_for_key(&run, "run_id");
    assert_eq!(value_for_key(&run, "status"), "completed");
    assert_eq!(value_for_key(&run, "result"), "cli answer");

    let replay = run_cli(
        [
            "display", "replay", "--store", db_arg, "--run", run_id, "--after", "0",
        ],
        ReplayRuntime::new("unused for replay"),
    )
    .expect("display replay succeeds");

    assert_eq!(
        replay.lines().collect::<Vec<_>>(),
        vec![
            "1 progress",
            "2 assistant_text_delta text=cli answer",
            "3 final_result result=cli answer",
        ]
    );
}

fn value_for_key<'a>(output: &'a str, key: &str) -> &'a str {
    output
        .lines()
        .find_map(|line| line.strip_prefix(&format!("{key}=")))
        .unwrap_or_else(|| panic!("missing {key}=... in output: {output}"))
}
