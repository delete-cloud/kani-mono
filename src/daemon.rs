use std::path::{Path, PathBuf};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc,
};
use std::thread::{self, JoinHandle};

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
    #[error("daemon process is stopped")]
    Stopped,
    #[error("daemon process failed to respond")]
    ResponseDropped,
    #[error("daemon process thread panicked")]
    ThreadPanicked,
}

pub struct LocalDaemon<R> {
    store_path: PathBuf,
    runtime: R,
}

enum DaemonCommand {
    CreateSession {
        target: RunTarget,
        reply: mpsc::Sender<Result<SessionRecord, DaemonError>>,
    },
    StartRun {
        session_id: String,
        input: String,
        reply: mpsc::Sender<Result<RunRecord, DaemonError>>,
    },
    ReplayDisplayEvents {
        run_id: String,
        after_sequence: u64,
        reply: mpsc::Sender<Result<Vec<DisplayEvent>, DaemonError>>,
    },
    Stop {
        reply: mpsc::Sender<Result<(), DaemonError>>,
    },
}

pub struct LocalDaemonProcess {
    sender: mpsc::Sender<DaemonCommand>,
    running: Arc<AtomicBool>,
    join_handle: std::sync::Mutex<Option<JoinHandle<()>>>,
}

impl LocalDaemonProcess {
    pub fn start<R>(store_path: impl AsRef<Path>, runtime: R) -> Result<Self, DaemonError>
    where
        R: AgentRuntime + Clone + Send + 'static,
    {
        let store_path = store_path.as_ref().to_path_buf();
        SqliteControlPlaneStore::open(&store_path)?;

        let (sender, receiver) = mpsc::channel();
        let running = Arc::new(AtomicBool::new(true));
        let actor_running = Arc::clone(&running);
        let join_handle = thread::spawn(move || {
            let mut daemon = match LocalDaemon::open(store_path, runtime) {
                Ok(daemon) => daemon,
                Err(_) => {
                    actor_running.store(false, Ordering::SeqCst);
                    return;
                }
            };

            for command in receiver {
                match command {
                    DaemonCommand::CreateSession { target, reply } => {
                        let _ = reply.send(daemon.create_session(target));
                    }
                    DaemonCommand::StartRun {
                        session_id,
                        input,
                        reply,
                    } => {
                        let _ = reply.send(daemon.start_run(&session_id, input));
                    }
                    DaemonCommand::ReplayDisplayEvents {
                        run_id,
                        after_sequence,
                        reply,
                    } => {
                        let _ = reply.send(daemon.replay_display_events(&run_id, after_sequence));
                    }
                    DaemonCommand::Stop { reply } => {
                        actor_running.store(false, Ordering::SeqCst);
                        let _ = reply.send(Ok(()));
                        break;
                    }
                }
            }

            actor_running.store(false, Ordering::SeqCst);
        });

        Ok(Self {
            sender,
            running,
            join_handle: std::sync::Mutex::new(Some(join_handle)),
        })
    }

    pub fn client(&self) -> LocalDaemonClient {
        LocalDaemonClient {
            sender: self.sender.clone(),
        }
    }

    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }

    pub fn stop(&self) -> Result<(), DaemonError> {
        if !self.is_running() {
            return Ok(());
        }

        let result = request(&self.sender, |reply| DaemonCommand::Stop { reply })?;
        let join_handle = self
            .join_handle
            .lock()
            .expect("daemon process join handle mutex poisoned")
            .take();
        if let Some(join_handle) = join_handle {
            join_handle
                .join()
                .map_err(|_| DaemonError::ThreadPanicked)?;
        }
        result
    }
}

impl Drop for LocalDaemonProcess {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

#[derive(Clone)]
pub struct LocalDaemonClient {
    sender: mpsc::Sender<DaemonCommand>,
}

impl LocalDaemonClient {
    pub fn create_session(
        &mut self,
        default_run_target: RunTarget,
    ) -> Result<SessionRecord, DaemonError> {
        request(&self.sender, |reply| DaemonCommand::CreateSession {
            target: default_run_target,
            reply,
        })?
    }

    pub fn start_run(
        &mut self,
        session_id: &str,
        input: impl Into<String>,
    ) -> Result<RunRecord, DaemonError> {
        request(&self.sender, |reply| DaemonCommand::StartRun {
            session_id: session_id.to_string(),
            input: input.into(),
            reply,
        })?
    }

    pub fn replay_display_events(
        &mut self,
        run_id: &str,
        after_sequence: u64,
    ) -> Result<Vec<DisplayEvent>, DaemonError> {
        request(&self.sender, |reply| DaemonCommand::ReplayDisplayEvents {
            run_id: run_id.to_string(),
            after_sequence,
            reply,
        })?
    }
}

fn request<T>(
    sender: &mpsc::Sender<DaemonCommand>,
    build: impl FnOnce(mpsc::Sender<T>) -> DaemonCommand,
) -> Result<T, DaemonError> {
    let (reply, receiver) = mpsc::channel();
    sender
        .send(build(reply))
        .map_err(|_| DaemonError::Stopped)?;
    receiver.recv().map_err(|_| DaemonError::ResponseDropped)
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
