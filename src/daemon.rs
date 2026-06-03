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
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum DaemonError {
    #[error(transparent)]
    ControlPlane(#[from] ControlPlaneError),
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("ipc error: {0}")]
    Ipc(String),
    #[error("daemon process is stopped")]
    Stopped,
    #[error("daemon process failed to respond")]
    ResponseDropped,
    #[error("daemon process thread panicked")]
    ThreadPanicked,
}

#[derive(Debug, Serialize, Deserialize)]
enum IpcRequest {
    Ping,
    Shutdown,
    CreateSession { target: RunTarget },
    StartRun { session_id: String, input: String },
    ReplayDisplayEvents { run_id: String, after_sequence: u64 },
}

#[derive(Debug, Serialize, Deserialize)]
enum IpcResponse {
    Pong,
    Stopping,
    Session { session: SessionRecord },
    Run { run: RunRecord },
    DisplayEvents { events: Vec<DisplayEvent> },
    Error { message: String },
}

pub struct LocalDaemon<R> {
    store_path: PathBuf,
    runtime: R,
}

#[cfg(unix)]
pub struct LocalDaemonIpcServer {
    socket_path: PathBuf,
    running: Arc<AtomicBool>,
    join_handle: std::sync::Mutex<Option<JoinHandle<()>>>,
    process: LocalDaemonProcess,
}

#[cfg(unix)]
impl LocalDaemonIpcServer {
    pub fn start<R>(
        socket_path: impl AsRef<Path>,
        store_path: impl AsRef<Path>,
        runtime: R,
    ) -> Result<Self, DaemonError>
    where
        R: AgentRuntime + Clone + Send + 'static,
    {
        use std::os::unix::net::UnixListener;

        let socket_path = socket_path.as_ref().to_path_buf();
        if socket_path.exists() {
            std::fs::remove_file(&socket_path)?;
        }
        let listener = UnixListener::bind(&socket_path)?;
        listener.set_nonblocking(true)?;

        let process = LocalDaemonProcess::start(store_path, runtime)?;
        let mut client = process.client();
        let running = Arc::new(AtomicBool::new(true));
        let server_running = Arc::clone(&running);
        let join_handle = thread::spawn(move || {
            while server_running.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((stream, _addr)) => {
                        if matches!(
                            serve_ipc_connection(&mut client, stream),
                            Ok(IpcConnectionOutcome::Shutdown)
                        ) {
                            server_running.store(false, Ordering::SeqCst);
                        }
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(std::time::Duration::from_millis(10));
                    }
                    Err(_) => {
                        server_running.store(false, Ordering::SeqCst);
                    }
                }
            }
        });

        Ok(Self {
            socket_path,
            running,
            join_handle: std::sync::Mutex::new(Some(join_handle)),
            process,
        })
    }

    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }

    pub fn stop(&self) -> Result<(), DaemonError> {
        self.running.store(false, Ordering::SeqCst);
        let join_handle = self
            .join_handle
            .lock()
            .expect("ipc server join handle mutex poisoned")
            .take();
        if let Some(join_handle) = join_handle {
            join_handle
                .join()
                .map_err(|_| DaemonError::ThreadPanicked)?;
        }
        self.process.stop()?;
        if self.socket_path.exists() {
            std::fs::remove_file(&self.socket_path)?;
        }
        Ok(())
    }
}

#[cfg(unix)]
impl Drop for LocalDaemonIpcServer {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

#[cfg(unix)]
pub struct LocalDaemonIpcClient {
    socket_path: PathBuf,
}

#[cfg(unix)]
impl LocalDaemonIpcClient {
    pub fn connect(socket_path: impl AsRef<Path>) -> Result<Self, DaemonError> {
        let socket_path = socket_path.as_ref().to_path_buf();
        Ok(Self { socket_path })
    }

    pub fn ping(&mut self) -> Result<(), DaemonError> {
        match self.send_request(IpcRequest::Ping)? {
            IpcResponse::Pong => Ok(()),
            response => Err(unexpected_ipc_response("pong", response)),
        }
    }

    pub fn shutdown(&mut self) -> Result<(), DaemonError> {
        match self.send_request(IpcRequest::Shutdown)? {
            IpcResponse::Stopping => Ok(()),
            response => Err(unexpected_ipc_response("stopping", response)),
        }
    }

    pub fn create_session(
        &mut self,
        default_run_target: RunTarget,
    ) -> Result<SessionRecord, DaemonError> {
        match self.send_request(IpcRequest::CreateSession {
            target: default_run_target,
        })? {
            IpcResponse::Session { session } => Ok(session),
            response => Err(unexpected_ipc_response("session", response)),
        }
    }

    pub fn start_run(
        &mut self,
        session_id: &str,
        input: impl Into<String>,
    ) -> Result<RunRecord, DaemonError> {
        match self.send_request(IpcRequest::StartRun {
            session_id: session_id.to_string(),
            input: input.into(),
        })? {
            IpcResponse::Run { run } => Ok(run),
            response => Err(unexpected_ipc_response("run", response)),
        }
    }

    pub fn replay_display_events(
        &mut self,
        run_id: &str,
        after_sequence: u64,
    ) -> Result<Vec<DisplayEvent>, DaemonError> {
        match self.send_request(IpcRequest::ReplayDisplayEvents {
            run_id: run_id.to_string(),
            after_sequence,
        })? {
            IpcResponse::DisplayEvents { events } => Ok(events),
            response => Err(unexpected_ipc_response("display events", response)),
        }
    }

    fn send_request(&self, request: IpcRequest) -> Result<IpcResponse, DaemonError> {
        let request = serde_json::to_string(&request)?;
        let mut last_error = None;
        for _attempt in 0..5 {
            match self.send_serialized_request(&request) {
                Ok(response) => return Ok(response),
                Err(error) if is_transient_ipc_io_error(&error) => {
                    last_error = Some(error);
                    thread::sleep(std::time::Duration::from_millis(10));
                }
                Err(error) => return Err(error),
            }
        }
        Err(last_error.expect("transient ipc retry loop records last error"))
    }

    fn send_serialized_request(&self, request: &str) -> Result<IpcResponse, DaemonError> {
        use std::io::{Read, Write};
        use std::net::Shutdown;
        use std::os::unix::net::UnixStream;

        let mut stream = UnixStream::connect(&self.socket_path)?;
        stream.write_all(request.as_bytes())?;
        stream.shutdown(Shutdown::Write)?;

        let mut response = String::new();
        stream.read_to_string(&mut response)?;
        let response: IpcResponse = serde_json::from_str(&response)?;
        match response {
            IpcResponse::Error { message } => Err(DaemonError::Ipc(message)),
            response => Ok(response),
        }
    }
}

#[cfg(unix)]
fn is_transient_ipc_io_error(error: &DaemonError) -> bool {
    matches!(
        error,
        DaemonError::Io(io_error)
            if matches!(
                io_error.kind(),
                std::io::ErrorKind::BrokenPipe
                    | std::io::ErrorKind::ConnectionRefused
                    | std::io::ErrorKind::NotFound
                    | std::io::ErrorKind::WouldBlock
            )
    )
}

#[cfg(unix)]
enum IpcConnectionOutcome {
    Continue,
    Shutdown,
}

#[cfg(unix)]
fn serve_ipc_connection(
    client: &mut LocalDaemonClient,
    mut stream: std::os::unix::net::UnixStream,
) -> Result<IpcConnectionOutcome, DaemonError> {
    use std::io::{Read, Write};

    let mut request = String::new();
    stream.read_to_string(&mut request)?;
    if request.trim().is_empty() {
        return Ok(IpcConnectionOutcome::Continue);
    }
    let request: IpcRequest = serde_json::from_str(&request)?;
    let (response, outcome) = match request {
        IpcRequest::Ping => (IpcResponse::Pong, IpcConnectionOutcome::Continue),
        IpcRequest::Shutdown => (IpcResponse::Stopping, IpcConnectionOutcome::Shutdown),
        IpcRequest::CreateSession { target } => match client.create_session(target) {
            Ok(session) => (
                IpcResponse::Session { session },
                IpcConnectionOutcome::Continue,
            ),
            Err(error) => (
                IpcResponse::Error {
                    message: error.to_string(),
                },
                IpcConnectionOutcome::Continue,
            ),
        },
        IpcRequest::StartRun { session_id, input } => match client.start_run(&session_id, input) {
            Ok(run) => (IpcResponse::Run { run }, IpcConnectionOutcome::Continue),
            Err(error) => (
                IpcResponse::Error {
                    message: error.to_string(),
                },
                IpcConnectionOutcome::Continue,
            ),
        },
        IpcRequest::ReplayDisplayEvents {
            run_id,
            after_sequence,
        } => match client.replay_display_events(&run_id, after_sequence) {
            Ok(events) => (
                IpcResponse::DisplayEvents { events },
                IpcConnectionOutcome::Continue,
            ),
            Err(error) => (
                IpcResponse::Error {
                    message: error.to_string(),
                },
                IpcConnectionOutcome::Continue,
            ),
        },
    };
    stream.write_all(serde_json::to_string(&response)?.as_bytes())?;
    Ok(outcome)
}

#[cfg(unix)]
fn unexpected_ipc_response(expected: &str, response: IpcResponse) -> DaemonError {
    DaemonError::Ipc(format!(
        "expected {expected} response, received {response:?}"
    ))
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
