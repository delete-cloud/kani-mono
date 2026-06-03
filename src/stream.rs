use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum RuntimeEventKind {
    RunStarted,
    ModelDelta,
    ToolCallStarted,
    ApprovalRequested,
    CheckpointCreated,
    RunCompleted,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RuntimeEvent {
    pub sequence: u64,
    pub event_id: String,
    pub run_id: String,
    pub kind: RuntimeEventKind,
    pub payload: Value,
}

impl RuntimeEvent {
    pub fn new(
        event_id: impl Into<String>,
        run_id: impl Into<String>,
        kind: RuntimeEventKind,
        payload: Value,
    ) -> Self {
        Self {
            sequence: 0,
            event_id: event_id.into(),
            run_id: run_id.into(),
            kind,
            payload,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum DisplayEventKind {
    AssistantTextDelta,
    ToolStatus,
    ApprovalPrompt,
    Progress,
    FinalResult,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DisplayEvent {
    pub sequence: u64,
    pub run_id: String,
    pub kind: DisplayEventKind,
    pub payload: Value,
}

#[derive(Default)]
pub struct EventLog {
    runtime_events: Vec<RuntimeEvent>,
    runtime_event_by_id: HashMap<String, usize>,
    display_events: Vec<DisplayEvent>,
}

impl EventLog {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn append_runtime(&mut self, mut event: RuntimeEvent) -> RuntimeEvent {
        if let Some(index) = self.runtime_event_by_id.get(&event.event_id) {
            return self.runtime_events[*index].clone();
        }

        event.sequence = self.runtime_events.len() as u64 + 1;
        self.runtime_event_by_id
            .insert(event.event_id.clone(), self.runtime_events.len());
        self.runtime_events.push(event.clone());
        event
    }

    pub fn replay_runtime(&self, run_id: &str, after_sequence: u64) -> Vec<RuntimeEvent> {
        self.runtime_events
            .iter()
            .filter(|event| event.run_id == run_id && event.sequence > after_sequence)
            .cloned()
            .collect()
    }

    pub fn project_runtime_event(&mut self, event: &RuntimeEvent) -> DisplayEvent {
        let display = match event.kind {
            RuntimeEventKind::ModelDelta => DisplayEvent {
                sequence: self.display_events.len() as u64 + 1,
                run_id: event.run_id.clone(),
                kind: DisplayEventKind::AssistantTextDelta,
                payload: json!({
                    "text": required_text_payload(&event.payload),
                }),
            },
            RuntimeEventKind::ApprovalRequested => DisplayEvent {
                sequence: self.display_events.len() as u64 + 1,
                run_id: event.run_id.clone(),
                kind: DisplayEventKind::ApprovalPrompt,
                payload: event.payload.clone(),
            },
            RuntimeEventKind::ToolCallStarted => DisplayEvent {
                sequence: self.display_events.len() as u64 + 1,
                run_id: event.run_id.clone(),
                kind: DisplayEventKind::ToolStatus,
                payload: event.payload.clone(),
            },
            RuntimeEventKind::RunStarted => DisplayEvent {
                sequence: self.display_events.len() as u64 + 1,
                run_id: event.run_id.clone(),
                kind: DisplayEventKind::Progress,
                payload: event.payload.clone(),
            },
            RuntimeEventKind::CheckpointCreated | RuntimeEventKind::RunCompleted => DisplayEvent {
                sequence: self.display_events.len() as u64 + 1,
                run_id: event.run_id.clone(),
                kind: DisplayEventKind::FinalResult,
                payload: event.payload.clone(),
            },
        };
        self.display_events.push(display.clone());
        display
    }

    pub fn replay_display(&self, run_id: &str, after_sequence: u64) -> Vec<DisplayEvent> {
        self.display_events
            .iter()
            .filter(|event| event.run_id == run_id && event.sequence > after_sequence)
            .cloned()
            .collect()
    }
}

fn required_text_payload(payload: &Value) -> &str {
    payload
        .get("text")
        .and_then(Value::as_str)
        .expect("RuntimeEventKind::ModelDelta payload must contain string field text")
}
