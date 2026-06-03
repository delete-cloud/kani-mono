use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use thiserror::Error;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum WorkspaceRef {
    LocalPath { path: String },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ExecutorRef {
    LocalDaemon,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct IsolationPolicy {
    pub filesystem: String,
    pub network: String,
    pub secrets: String,
}

impl IsolationPolicy {
    pub fn default_local_sandbox() -> Self {
        Self {
            filesystem: "workspace_scoped".to_string(),
            network: "restricted".to_string(),
            secrets: "explicit_allowlist".to_string(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct RunConstraints {
    pub max_steps: Option<u32>,
    pub timeout_seconds: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunTarget {
    pub workspace: WorkspaceRef,
    pub executor: ExecutorRef,
    pub isolation: IsolationPolicy,
    pub constraints: RunConstraints,
}

impl RunTarget {
    pub fn local_daemon(path: impl Into<String>) -> Self {
        Self {
            workspace: WorkspaceRef::LocalPath { path: path.into() },
            executor: ExecutorRef::LocalDaemon,
            isolation: IsolationPolicy::default_local_sandbox(),
            constraints: RunConstraints::default(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum RunStatus {
    Queued,
    Running,
    WaitingApproval,
    Cancelling,
    Cancelled,
    Completed,
    Failed,
    Interrupted,
}

impl RunStatus {
    pub fn is_active(&self) -> bool {
        matches!(
            self,
            Self::Queued | Self::Running | Self::WaitingApproval | Self::Cancelling
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResumeSnapshot {
    pub run_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionRecord {
    pub session_id: String,
    pub default_run_target: RunTarget,
    pub current_run_id: Option<String>,
    pub tape_id: String,
    pub status: String,
    pub metadata: HashMap<String, String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunRecord {
    pub run_id: String,
    pub session_id: String,
    pub target: RunTarget,
    pub status: RunStatus,
    pub input: String,
    pub result: Option<String>,
    pub error: Option<String>,
    pub resume_from: Option<ResumeSnapshot>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ApprovalStatus {
    Pending,
    Approved,
    Denied,
    Expired,
    Cancelled,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ApprovalRecord {
    pub approval_id: String,
    pub run_id: String,
    pub tool_call_ref: String,
    pub status: ApprovalStatus,
    pub request_payload: Value,
    pub decision: Option<String>,
    pub feedback: Option<String>,
    pub resolved_at: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CancelIntent {
    pub cancel_id: String,
    pub run_id: String,
    pub reason: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CheckpointRecord {
    pub checkpoint_id: String,
    pub run_id: String,
    pub tape_id: String,
    pub visible_head_seq: u64,
    pub epoch: u64,
    pub context_digest: String,
    pub metadata: Value,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum SessionError {
    #[error("session not found")]
    SessionNotFound,
    #[error("run not found")]
    RunNotFound,
    #[error("session has no previous run to resume")]
    NoPreviousRun,
    #[error("latest run is still active")]
    LatestRunActive,
    #[error("approval not found")]
    ApprovalNotFound,
}
