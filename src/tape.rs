use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ActiveState {
    Active,
    Superseded,
    Inactive,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum EntryKind {
    Message,
    ToolCall,
    ToolResult,
    Anchor,
    RestoreMarker,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TapeEntry {
    pub seq: u64,
    pub entry_id: String,
    pub kind: EntryKind,
    pub payload: Value,
    pub meta: Value,
    pub run_id: String,
    pub epoch: u64,
    pub active_state: ActiveState,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RestoreMarker {
    pub seq: u64,
    pub checkpoint_id: String,
    pub restore_to_seq: u64,
    pub previous_head_seq: u64,
    pub restore_reason: String,
    pub epoch: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct KvCacheKey {
    pub tape_id: String,
    pub epoch: u64,
    pub visible_head_seq: u64,
    pub context_digest: String,
    pub model_config_digest: String,
}

#[derive(Clone, Debug)]
pub struct Tape {
    tape_id: String,
    entries: Vec<TapeEntry>,
    current_epoch: u64,
}

impl Tape {
    pub fn new(tape_id: impl Into<String>) -> Self {
        Self {
            tape_id: tape_id.into(),
            entries: Vec::new(),
            current_epoch: 0,
        }
    }

    pub fn tape_id(&self) -> &str {
        &self.tape_id
    }

    pub fn entries(&self) -> &[TapeEntry] {
        &self.entries
    }

    pub fn append(
        &mut self,
        run_id: impl Into<String>,
        kind: EntryKind,
        payload: Value,
    ) -> TapeEntry {
        let entry = TapeEntry {
            seq: self.next_seq(),
            entry_id: Uuid::new_v4().to_string(),
            kind,
            payload,
            meta: json!({}),
            run_id: run_id.into(),
            epoch: self.current_epoch,
            active_state: ActiveState::Active,
        };
        self.entries.push(entry.clone());
        entry
    }

    pub fn restore_to_checkpoint(
        &mut self,
        checkpoint_id: impl Into<String>,
        restore_to_seq: u64,
        restore_reason: impl Into<String>,
    ) -> RestoreMarker {
        let previous_head_seq = self
            .entries
            .last()
            .map(|entry| entry.seq)
            .unwrap_or(restore_to_seq);
        let checkpoint_id = checkpoint_id.into();
        let restore_reason = restore_reason.into();

        for entry in &mut self.entries {
            if entry.seq > restore_to_seq && entry.active_state == ActiveState::Active {
                entry.active_state = ActiveState::Superseded;
            }
        }

        self.current_epoch += 1;
        let marker = RestoreMarker {
            seq: self.next_seq(),
            checkpoint_id,
            restore_to_seq,
            previous_head_seq,
            restore_reason,
            epoch: self.current_epoch,
        };
        let marker_entry = TapeEntry {
            seq: marker.seq,
            entry_id: Uuid::new_v4().to_string(),
            kind: EntryKind::RestoreMarker,
            payload: json!({
                "checkpoint_id": marker.checkpoint_id,
                "restore_to_seq": marker.restore_to_seq,
                "previous_head_seq": marker.previous_head_seq,
                "restore_reason": marker.restore_reason,
                "epoch": marker.epoch,
            }),
            meta: json!({"active_view_marker": true}),
            run_id: String::new(),
            epoch: marker.epoch,
            active_state: ActiveState::Active,
        };
        self.entries.push(marker_entry);
        marker
    }

    pub fn active_view(&self) -> Vec<TapeEntry> {
        self.entries
            .iter()
            .filter(|entry| entry.active_state == ActiveState::Active)
            .cloned()
            .collect()
    }

    pub fn kv_cache_key(
        &self,
        context_digest: impl Into<String>,
        model_config_digest: impl Into<String>,
    ) -> KvCacheKey {
        KvCacheKey {
            tape_id: self.tape_id.clone(),
            epoch: self.current_epoch,
            visible_head_seq: self.visible_head_seq(),
            context_digest: context_digest.into(),
            model_config_digest: model_config_digest.into(),
        }
    }

    fn visible_head_seq(&self) -> u64 {
        self.entries
            .iter()
            .rev()
            .find(|entry| entry.active_state == ActiveState::Active)
            .map(|entry| entry.seq)
            .unwrap_or(0)
    }

    fn next_seq(&self) -> u64 {
        self.entries.len() as u64
    }
}
