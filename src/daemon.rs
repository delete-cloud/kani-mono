use std::path::{Path, PathBuf};

use crate::controlplane::{ControlPlaneError, DurableRunCoordinator, DurableSessionService};
use crate::executor::LocalDaemonExecutor;
use crate::runtime::AgentRuntime;
use crate::session::{RunRecord, RunTarget, SessionRecord};
use crate::storage::{SqliteControlPlaneStore, StoreError};
use crate::stream::DisplayEvent;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum DaemonError {
    #[error(transparent)]
    ControlPlane(#[from] ControlPlaneError),
    #[error(transparent)]
    Store(#[from] StoreError),
}

pub struct LocalDaemon<R> {
    store_path: PathBuf,
    runtime: R,
}

impl<R> LocalDaemon<R>
where
    R: AgentRuntime,
{
    pub fn open(store_path: impl AsRef<Path>, runtime: R) -> Result<Self, DaemonError> {
        let store_path = store_path.as_ref().to_path_buf();
        SqliteControlPlaneStore::open(&store_path)?;
        Ok(Self {
            store_path,
            runtime,
        })
    }

    pub fn create_session(
        &mut self,
        default_run_target: RunTarget,
    ) -> Result<SessionRecord, DaemonError> {
        let store = self.open_store()?;
        let mut service = DurableSessionService::new(store);
        Ok(service.create_session(default_run_target)?)
    }

    pub fn session(&self, session_id: &str) -> Result<Option<SessionRecord>, DaemonError> {
        let store = self.open_store()?;
        Ok(store.load_session(session_id)?)
    }

    pub fn run(&self, run_id: &str) -> Result<Option<RunRecord>, DaemonError> {
        let store = self.open_store()?;
        Ok(store.load_run(run_id)?)
    }

    pub fn start_run(
        &mut self,
        session_id: &str,
        input: impl Into<String>,
    ) -> Result<RunRecord, DaemonError>
    where
        R: Clone,
    {
        let store = self.open_store()?;
        let mut service = DurableSessionService::new(store);
        let executor = LocalDaemonExecutor::new(self.runtime.clone());
        let mut coordinator = DurableRunCoordinator::new(executor);
        Ok(coordinator.start_run(&mut service, session_id, input)?)
    }

    pub fn replay_display_events(
        &self,
        run_id: &str,
        after_sequence: u64,
    ) -> Result<Vec<DisplayEvent>, DaemonError> {
        let store = self.open_store()?;
        Ok(store.replay_display_events(run_id, after_sequence)?)
    }

    fn open_store(&self) -> Result<SqliteControlPlaneStore, StoreError> {
        SqliteControlPlaneStore::open(&self.store_path)
    }
}
