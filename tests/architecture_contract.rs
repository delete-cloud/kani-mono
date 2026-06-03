use kani_mono::controlplane::SessionService;
use kani_mono::session::{ApprovalStatus, RunStatus, RunTarget};
use kani_mono::stream::{DisplayEventKind, EventLog, RuntimeEvent, RuntimeEventKind};
use kani_mono::tape::{ActiveState, EntryKind, Tape};
use pretty_assertions::assert_eq;
use serde_json::json;

#[test]
fn restore_appends_marker_and_switches_active_view_without_truncating_tape() {
    let mut tape = Tape::new("tape-1");
    let checkpoint_entry = tape.append(
        "run-1",
        EntryKind::Message,
        json!({"role": "user", "content": "start"}),
    );
    let superseded_entry = tape.append(
        "run-1",
        EntryKind::Message,
        json!({"role": "assistant", "content": "wrong path"}),
    );

    let marker = tape.restore_to_checkpoint(
        "checkpoint-1",
        checkpoint_entry.seq,
        "user requested rollback",
    );
    let continued_entry = tape.append(
        "run-2",
        EntryKind::Message,
        json!({"role": "assistant", "content": "correct path"}),
    );
    let cache_key = tape.kv_cache_key("context-digest-1", "model-config-1");

    assert_eq!(tape.entries().len(), 4);
    assert_eq!(marker.seq, 2);
    assert_eq!(marker.previous_head_seq, superseded_entry.seq);
    assert_eq!(continued_entry.epoch, marker.epoch);
    assert_eq!(tape.entries()[1].active_state, ActiveState::Superseded);
    assert_eq!(
        tape.active_view()
            .iter()
            .map(|entry| entry.seq)
            .collect::<Vec<_>>(),
        vec![0, 2, 3]
    );
    assert_ne!(tape.entries()[0].epoch, tape.entries()[2].epoch);
    assert_eq!(cache_key.tape_id, "tape-1");
    assert_eq!(cache_key.epoch, marker.epoch);
    assert_eq!(cache_key.visible_head_seq, continued_entry.seq);
    assert_eq!(cache_key.context_digest, "context-digest-1");
    assert_eq!(cache_key.model_config_digest, "model-config-1");
}

#[test]
fn resume_creates_new_run_from_durable_session_context() {
    let mut service = SessionService::new();
    let target = RunTarget::local_daemon("/workspace");
    let session = service.create_session(target.clone());
    let first = service
        .start_run(&session.session_id, "implement contract")
        .expect("first run starts");
    service
        .finish_run(&first.run_id, RunStatus::Interrupted)
        .expect("first run can be interrupted");

    let resumed = service
        .resume_session(&session.session_id, "continue from durable context")
        .expect("resume creates a new run");

    assert_ne!(resumed.run_id, first.run_id);
    assert_eq!(resumed.session_id, session.session_id);
    assert_eq!(resumed.target, target);
    assert_eq!(
        resumed
            .resume_from
            .as_ref()
            .map(|resume| resume.run_id.as_str()),
        Some(first.run_id.as_str())
    );
    assert_eq!(
        service
            .session(&session.session_id)
            .and_then(|session| session.current_run_id.as_deref()),
        Some(resumed.run_id.as_str())
    );
}

#[test]
fn resume_rejects_active_runs_instead_of_retrying_them() {
    let mut service = SessionService::new();
    let session = service.create_session(RunTarget::local_daemon("/workspace"));
    let active = service
        .start_run(&session.session_id, "still running")
        .expect("active run starts");

    let error = service
        .resume_session(&session.session_id, "must not retry active run")
        .expect_err("active run blocks resume");

    assert_eq!(error.to_string(), "latest run is still active");
    assert_eq!(
        service
            .session(&session.session_id)
            .and_then(|session| session.current_run_id.as_deref()),
        Some(active.run_id.as_str())
    );
}

#[test]
fn runtime_events_dedupe_by_event_id_and_display_events_are_projection_only() {
    let mut log = EventLog::new();
    let runtime_event = RuntimeEvent::new(
        "event-1",
        "run-1",
        RuntimeEventKind::ModelDelta,
        json!({"text": "hello"}),
    );

    let first = log.append_runtime(runtime_event.clone());
    let duplicate = log.append_runtime(RuntimeEvent::new(
        "event-1",
        "run-1",
        RuntimeEventKind::ModelDelta,
        json!({"text": "duplicate must not overwrite"}),
    ));
    let display = log.project_runtime_event(&runtime_event);
    let duplicate_display = log.project_runtime_event(&duplicate);

    assert_eq!(first.sequence, 1);
    assert_eq!(duplicate.sequence, 1);
    assert_eq!(
        log.replay_runtime("run-1", 0)
            .iter()
            .map(|event| event.event_id.as_str())
            .collect::<Vec<_>>(),
        vec!["event-1"]
    );
    assert_eq!(display.kind, DisplayEventKind::AssistantTextDelta);
    assert_eq!(display.payload, json!({"text": "hello"}));
    assert_eq!(duplicate_display, display);
    assert_eq!(log.replay_display("run-1", 0).len(), 1);
}

#[test]
fn approval_and_cancel_are_durable_controlplane_intents() {
    let mut service = SessionService::new();
    let session = service.create_session(RunTarget::local_daemon("/workspace"));
    let run = service
        .start_run(&session.session_id, "needs approval")
        .expect("run starts");

    let approval = service
        .request_approval(&run.run_id, "tool-call-1", json!({"cmd": "rm -rf target"}))
        .expect("approval request is persisted");
    let approved = service
        .resolve_approval(&approval.approval_id, true, "allow test cleanup")
        .expect("approval decision is persisted");
    let duplicate_decision = service
        .resolve_approval(&approval.approval_id, false, "must not overwrite")
        .expect("resolved approval remains idempotent");

    assert_eq!(approval.status, ApprovalStatus::Pending);
    assert_eq!(approved.status, ApprovalStatus::Approved);
    assert_eq!(duplicate_decision.decision.as_deref(), Some("approved"));
    assert_eq!(
        duplicate_decision.feedback.as_deref(),
        Some("allow test cleanup")
    );

    let cancel = service
        .request_cancel(&run.run_id, "user requested")
        .expect("cancel intent is persisted");
    let cancelling_run = service.run(&run.run_id).expect("run remains durable");

    assert_eq!(cancel.run_id, run.run_id);
    assert_eq!(cancel.reason, "user requested");
    assert_eq!(cancelling_run.status, RunStatus::Cancelling);
}
