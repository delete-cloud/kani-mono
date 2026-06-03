use std::collections::HashMap;

use crate::executor::Executor;
use crate::session::{
    ApprovalRecord, ApprovalStatus, CancelIntent, ResumeSnapshot, RunRecord, RunStatus, RunTarget,
    SessionError, SessionRecord,
};
use crate::storage::{ControlPlaneStore, StoreError};
use crate::stream::{EventLog, RuntimeEvent};
use serde_json::Value;
use thiserror::Error;
use uuid::Uuid;

#[derive(Debug, Error)]
pub enum ControlPlaneError {
    #[error(transparent)]
    Session(#[from] SessionError),
    #[error(transparent)]
    Store(#[from] StoreError),
}

#[derive(Default)]
pub struct SessionService {
    sessions: HashMap<String, SessionRecord>,
    runs: HashMap<String, RunRecord>,
    approvals: HashMap<String, ApprovalRecord>,
    cancels: HashMap<String, CancelIntent>,
}

impl SessionService {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn create_session(&mut self, default_run_target: RunTarget) -> SessionRecord {
        let session_id = Uuid::new_v4().to_string();
        let session = SessionRecord {
            tape_id: format!("tape-{session_id}"),
            session_id: session_id.clone(),
            default_run_target,
            current_run_id: None,
            status: "active".to_string(),
            metadata: HashMap::new(),
        };
        self.sessions.insert(session_id, session.clone());
        session
    }

    pub fn session(&self, session_id: &str) -> Option<&SessionRecord> {
        self.sessions.get(session_id)
    }

    pub fn run(&self, run_id: &str) -> Option<&RunRecord> {
        self.runs.get(run_id)
    }

    pub fn start_run(
        &mut self,
        session_id: &str,
        input: impl Into<String>,
    ) -> Result<RunRecord, SessionError> {
        let session = self
            .sessions
            .get_mut(session_id)
            .ok_or(SessionError::SessionNotFound)?;
        let run = RunRecord {
            run_id: Uuid::new_v4().to_string(),
            session_id: session_id.to_string(),
            target: session.default_run_target.clone(),
            status: RunStatus::Running,
            input: input.into(),
            result: None,
            error: None,
            resume_from: None,
        };
        session.current_run_id = Some(run.run_id.clone());
        self.runs.insert(run.run_id.clone(), run.clone());
        Ok(run)
    }

    pub fn finish_run(
        &mut self,
        run_id: &str,
        status: RunStatus,
    ) -> Result<RunRecord, SessionError> {
        let run = self.runs.get_mut(run_id).ok_or(SessionError::RunNotFound)?;
        run.status = status;
        Ok(run.clone())
    }

    pub fn complete_run(
        &mut self,
        run_id: &str,
        result: impl Into<String>,
    ) -> Result<RunRecord, SessionError> {
        let run = self.runs.get_mut(run_id).ok_or(SessionError::RunNotFound)?;
        run.status = RunStatus::Completed;
        run.result = Some(result.into());
        Ok(run.clone())
    }

    pub fn resume_session(
        &mut self,
        session_id: &str,
        input: impl Into<String>,
    ) -> Result<RunRecord, SessionError> {
        let previous_run_id = self
            .sessions
            .get(session_id)
            .ok_or(SessionError::SessionNotFound)?
            .current_run_id
            .clone()
            .ok_or(SessionError::NoPreviousRun)?;
        let previous_run = self
            .runs
            .get(&previous_run_id)
            .ok_or(SessionError::RunNotFound)?;
        if previous_run.status.is_active() {
            return Err(SessionError::LatestRunActive);
        }

        let session = self
            .sessions
            .get_mut(session_id)
            .ok_or(SessionError::SessionNotFound)?;
        let run = RunRecord {
            run_id: Uuid::new_v4().to_string(),
            session_id: session_id.to_string(),
            target: session.default_run_target.clone(),
            status: RunStatus::Running,
            input: input.into(),
            result: None,
            error: None,
            resume_from: Some(ResumeSnapshot {
                run_id: previous_run.run_id.clone(),
            }),
        };
        session.current_run_id = Some(run.run_id.clone());
        self.runs.insert(run.run_id.clone(), run.clone());
        Ok(run)
    }

    pub fn request_approval(
        &mut self,
        run_id: &str,
        tool_call_ref: impl Into<String>,
        request_payload: Value,
    ) -> Result<ApprovalRecord, SessionError> {
        if !self.runs.contains_key(run_id) {
            return Err(SessionError::RunNotFound);
        }

        let approval = ApprovalRecord {
            approval_id: Uuid::new_v4().to_string(),
            run_id: run_id.to_string(),
            tool_call_ref: tool_call_ref.into(),
            status: ApprovalStatus::Pending,
            request_payload,
            decision: None,
            feedback: None,
            resolved_at: None,
        };
        self.approvals
            .insert(approval.approval_id.clone(), approval.clone());
        Ok(approval)
    }

    pub fn resolve_approval(
        &mut self,
        approval_id: &str,
        approved: bool,
        feedback: impl Into<String>,
    ) -> Result<ApprovalRecord, SessionError> {
        let approval = self
            .approvals
            .get_mut(approval_id)
            .ok_or(SessionError::ApprovalNotFound)?;
        if approval.resolved_at.is_some() {
            return Ok(approval.clone());
        }

        approval.status = if approved {
            ApprovalStatus::Approved
        } else {
            ApprovalStatus::Denied
        };
        approval.decision = Some(if approved { "approved" } else { "denied" }.to_string());
        approval.feedback = Some(feedback.into());
        approval.resolved_at = Some(Uuid::new_v4().to_string());
        Ok(approval.clone())
    }

    pub fn request_cancel(
        &mut self,
        run_id: &str,
        reason: impl Into<String>,
    ) -> Result<CancelIntent, SessionError> {
        let run = self.runs.get_mut(run_id).ok_or(SessionError::RunNotFound)?;
        run.status = RunStatus::Cancelling;
        let cancel = CancelIntent {
            cancel_id: Uuid::new_v4().to_string(),
            run_id: run_id.to_string(),
            reason: reason.into(),
        };
        self.cancels
            .insert(cancel.cancel_id.clone(), cancel.clone());
        Ok(cancel)
    }
}

pub trait RunControlPlane {
    type Error;

    fn start_run(&mut self, session_id: &str, input: String) -> Result<RunRecord, Self::Error>;
    fn complete_run(&mut self, run_id: &str, result: String) -> Result<RunRecord, Self::Error>;
}

impl RunControlPlane for SessionService {
    type Error = SessionError;

    fn start_run(&mut self, session_id: &str, input: String) -> Result<RunRecord, Self::Error> {
        SessionService::start_run(self, session_id, input)
    }

    fn complete_run(&mut self, run_id: &str, result: String) -> Result<RunRecord, Self::Error> {
        SessionService::complete_run(self, run_id, result)
    }
}

pub struct DurableSessionService<S> {
    store: S,
}

impl<S> DurableSessionService<S>
where
    S: ControlPlaneStore,
{
    pub fn new(store: S) -> Self {
        Self { store }
    }

    pub fn create_session(
        &mut self,
        default_run_target: RunTarget,
    ) -> Result<SessionRecord, ControlPlaneError> {
        let session_id = Uuid::new_v4().to_string();
        let session = SessionRecord {
            tape_id: format!("tape-{session_id}"),
            session_id: session_id.clone(),
            default_run_target,
            current_run_id: None,
            status: "active".to_string(),
            metadata: HashMap::new(),
        };
        self.store.save_session(&session)?;
        Ok(session)
    }

    pub fn session(&self, session_id: &str) -> Result<Option<SessionRecord>, ControlPlaneError> {
        Ok(self.store.load_session(session_id)?)
    }

    pub fn run(&self, run_id: &str) -> Result<Option<RunRecord>, ControlPlaneError> {
        Ok(self.store.load_run(run_id)?)
    }

    pub fn start_run(
        &mut self,
        session_id: &str,
        input: impl Into<String>,
    ) -> Result<RunRecord, ControlPlaneError> {
        let mut session = self
            .store
            .load_session(session_id)?
            .ok_or(SessionError::SessionNotFound)?;
        let run = RunRecord {
            run_id: Uuid::new_v4().to_string(),
            session_id: session_id.to_string(),
            target: session.default_run_target.clone(),
            status: RunStatus::Running,
            input: input.into(),
            result: None,
            error: None,
            resume_from: None,
        };
        session.current_run_id = Some(run.run_id.clone());
        self.store.save_session(&session)?;
        self.store.save_run(&run)?;
        Ok(run)
    }

    pub fn complete_run(
        &mut self,
        run_id: &str,
        result: impl Into<String>,
    ) -> Result<RunRecord, ControlPlaneError> {
        let mut run = self
            .store
            .load_run(run_id)?
            .ok_or(SessionError::RunNotFound)?;
        run.status = RunStatus::Completed;
        run.result = Some(result.into());
        self.store.save_run(&run)?;
        Ok(run)
    }

    pub fn request_approval(
        &mut self,
        run_id: &str,
        tool_call_ref: impl Into<String>,
        request_payload: Value,
    ) -> Result<ApprovalRecord, ControlPlaneError> {
        if self.store.load_run(run_id)?.is_none() {
            return Err(SessionError::RunNotFound.into());
        }

        let approval = ApprovalRecord {
            approval_id: Uuid::new_v4().to_string(),
            run_id: run_id.to_string(),
            tool_call_ref: tool_call_ref.into(),
            status: ApprovalStatus::Pending,
            request_payload,
            decision: None,
            feedback: None,
            resolved_at: None,
        };
        self.store.save_approval(&approval)?;
        Ok(approval)
    }

    pub fn resolve_approval(
        &mut self,
        approval_id: &str,
        approved: bool,
        feedback: impl Into<String>,
    ) -> Result<ApprovalRecord, ControlPlaneError> {
        let mut approval = self
            .store
            .load_approval(approval_id)?
            .ok_or(SessionError::ApprovalNotFound)?;
        if approval.resolved_at.is_none() {
            approval.status = if approved {
                ApprovalStatus::Approved
            } else {
                ApprovalStatus::Denied
            };
            approval.decision = Some(if approved { "approved" } else { "denied" }.to_string());
            approval.feedback = Some(feedback.into());
            approval.resolved_at = Some(Uuid::new_v4().to_string());
            self.store.save_approval(&approval)?;
        }
        Ok(approval)
    }

    pub fn request_cancel(
        &mut self,
        run_id: &str,
        reason: impl Into<String>,
    ) -> Result<CancelIntent, ControlPlaneError> {
        let mut run = self
            .store
            .load_run(run_id)?
            .ok_or(SessionError::RunNotFound)?;
        run.status = RunStatus::Cancelling;
        self.store.save_run(&run)?;

        let cancel = CancelIntent {
            cancel_id: Uuid::new_v4().to_string(),
            run_id: run_id.to_string(),
            reason: reason.into(),
        };
        self.store.save_cancel(&cancel)?;
        Ok(cancel)
    }

    pub fn persist_runtime_event(
        &mut self,
        event: RuntimeEvent,
    ) -> Result<RuntimeEvent, ControlPlaneError> {
        let event = self.store.append_runtime_event(event)?;
        self.store.project_display_event(&event)?;
        Ok(event)
    }
}

impl<S> RunControlPlane for DurableSessionService<S>
where
    S: ControlPlaneStore,
{
    type Error = ControlPlaneError;

    fn start_run(&mut self, session_id: &str, input: String) -> Result<RunRecord, Self::Error> {
        DurableSessionService::start_run(self, session_id, input)
    }

    fn complete_run(&mut self, run_id: &str, result: String) -> Result<RunRecord, Self::Error> {
        DurableSessionService::complete_run(self, run_id, result)
    }
}

pub struct RunCoordinator<E> {
    executor: E,
}

impl<E> RunCoordinator<E>
where
    E: Executor,
{
    pub fn new(executor: E) -> Self {
        Self { executor }
    }

    pub fn start_run<S>(
        &mut self,
        service: &mut S,
        events: &mut EventLog,
        session_id: &str,
        input: impl Into<String>,
    ) -> Result<RunRecord, S::Error>
    where
        S: RunControlPlane,
    {
        let run = service.start_run(session_id, input.into())?;
        let outcome = self.executor.execute(&run, events);
        service.complete_run(&run.run_id, outcome.final_text)
    }
}

pub struct DurableRunCoordinator<E> {
    executor: E,
}

impl<E> DurableRunCoordinator<E>
where
    E: Executor,
{
    pub fn new(executor: E) -> Self {
        Self { executor }
    }

    pub fn start_run<S>(
        &mut self,
        service: &mut DurableSessionService<S>,
        session_id: &str,
        input: impl Into<String>,
    ) -> Result<RunRecord, ControlPlaneError>
    where
        S: ControlPlaneStore,
    {
        let run = service.start_run(session_id, input)?;
        let mut events = EventLog::new();
        let outcome = self.executor.execute(&run, &mut events);
        for event in events.runtime_events() {
            service.persist_runtime_event(event.clone())?;
        }
        service.complete_run(&run.run_id, outcome.final_text)
    }
}
