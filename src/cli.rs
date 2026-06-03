use std::io::{Read, Write};
use std::path::PathBuf;
#[cfg(unix)]
use std::sync::mpsc;
#[cfg(unix)]
use std::time::Duration;

use crate::daemon::{DaemonError, LocalDaemon};
#[cfg(unix)]
use crate::daemon::{LocalDaemonIpcClient, LocalDaemonIpcServer};
use crate::runtime::AgentRuntime;
use crate::session::{RunRecord, RunStatus, RunTarget};
use crate::stream::{DisplayEvent, DisplayEventKind};
use serde_json::Value;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum CliError {
    #[error("{0}")]
    Usage(String),
    #[error("invalid --after value: {0}")]
    InvalidAfter(String),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Daemon(#[from] DaemonError),
}

pub fn run_cli<I, S, R>(args: I, runtime: R) -> Result<String, CliError>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
    R: AgentRuntime + Clone,
{
    let args = args
        .into_iter()
        .map(|arg| arg.as_ref().to_string())
        .collect::<Vec<_>>();
    let command = CliCommand::parse(&args)?;
    command.execute(runtime)
}

pub fn run_cli_stdio<I, S, R, Input, Output>(
    args: I,
    runtime: R,
    input: Input,
    mut output: Output,
) -> Result<(), CliError>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
    R: AgentRuntime + Clone + Send + 'static,
    Input: Read + Send + 'static,
    Output: Write,
{
    let args = args
        .into_iter()
        .map(|arg| arg.as_ref().to_string())
        .collect::<Vec<_>>();
    if is_daemon_serve(&args) {
        serve_daemon(&args[2..], runtime, input, output)
    } else if is_daemon_status(&args) {
        daemon_status(&args[2..], output)
    } else if is_daemon_stop(&args) {
        daemon_stop(&args[2..], output)
    } else {
        let output_text = run_cli(args.iter().map(String::as_str), runtime)?;
        if !output_text.is_empty() {
            writeln!(output, "{output_text}")?;
        }
        Ok(())
    }
}

#[derive(Debug, PartialEq, Eq)]
enum CliCommand {
    CreateSession {
        target: ClientTarget,
        workspace_path: String,
    },
    StartRun {
        target: ClientTarget,
        session_id: String,
        input: String,
    },
    RunStatus {
        target: ClientTarget,
        run_id: String,
    },
    ReplayDisplay {
        target: ClientTarget,
        run_id: String,
        after_sequence: u64,
    },
}

#[derive(Debug, PartialEq, Eq)]
enum ClientTarget {
    Store { store_path: PathBuf },
    Socket { socket_path: PathBuf },
}

fn is_daemon_serve(args: &[String]) -> bool {
    matches!(args, [scope, action, ..] if scope == "daemon" && action == "serve")
}

fn is_daemon_status(args: &[String]) -> bool {
    matches!(args, [scope, action, ..] if scope == "daemon" && action == "status")
}

fn is_daemon_stop(args: &[String]) -> bool {
    matches!(args, [scope, action, ..] if scope == "daemon" && action == "stop")
}

fn daemon_status<Output>(args: &[String], mut output: Output) -> Result<(), CliError>
where
    Output: Write,
{
    let socket_path = PathBuf::from(required_flag(args, "--socket")?);
    #[cfg(unix)]
    {
        let mut client = LocalDaemonIpcClient::connect(&socket_path)?;
        client.ping()?;
        writeln!(output, "daemon_status=running")?;
        writeln!(output, "socket={}", socket_path.display())?;
        output.flush()?;
        Ok(())
    }
    #[cfg(not(unix))]
    {
        let _ = socket_path;
        let _ = output;
        Err(CliError::Usage(
            "daemon status is only supported on Unix platforms".to_string(),
        ))
    }
}

fn daemon_stop<Output>(args: &[String], mut output: Output) -> Result<(), CliError>
where
    Output: Write,
{
    let socket_path = PathBuf::from(required_flag(args, "--socket")?);
    #[cfg(unix)]
    {
        let mut client = LocalDaemonIpcClient::connect(&socket_path)?;
        client.shutdown()?;
        writeln!(output, "daemon_status=stopping")?;
        writeln!(output, "socket={}", socket_path.display())?;
        output.flush()?;
        Ok(())
    }
    #[cfg(not(unix))]
    {
        let _ = socket_path;
        let _ = output;
        Err(CliError::Usage(
            "daemon stop is only supported on Unix platforms".to_string(),
        ))
    }
}

fn serve_daemon<R, Input, Output>(
    args: &[String],
    runtime: R,
    mut input: Input,
    mut output: Output,
) -> Result<(), CliError>
where
    R: AgentRuntime + Clone + Send + 'static,
    Input: Read + Send + 'static,
    Output: Write,
{
    let socket_path = PathBuf::from(required_flag(args, "--socket")?);
    let store_path = PathBuf::from(required_flag(args, "--store")?);
    #[cfg(unix)]
    {
        let server = LocalDaemonIpcServer::start(&socket_path, &store_path, runtime)?;
        writeln!(
            output,
            "daemon_ready socket={} store={}",
            socket_path.display(),
            store_path.display()
        )?;
        output.flush()?;

        let (stdin_done_sender, stdin_done_receiver) = mpsc::channel();
        std::thread::spawn(move || {
            let mut buffer = Vec::new();
            let result = input.read_to_end(&mut buffer).map(|_| ());
            let _ = stdin_done_sender.send(result);
        });
        while server.is_running() {
            match stdin_done_receiver.try_recv() {
                Ok(result) => {
                    result?;
                    break;
                }
                Err(mpsc::TryRecvError::Empty) => std::thread::sleep(Duration::from_millis(20)),
                Err(mpsc::TryRecvError::Disconnected) => break,
            }
        }
        server.stop()?;
        writeln!(output, "daemon_stopped")?;
        output.flush()?;
        Ok(())
    }
    #[cfg(not(unix))]
    {
        let _ = runtime;
        let _ = input;
        let _ = output;
        Err(CliError::Usage(
            "daemon serve is only supported on Unix platforms".to_string(),
        ))
    }
}

impl CliCommand {
    fn parse(args: &[String]) -> Result<Self, CliError> {
        match args {
            [scope, action, rest @ ..] if scope == "session" && action == "create" => {
                Ok(Self::CreateSession {
                    target: required_client_target(rest)?,
                    workspace_path: required_flag(rest, "--workspace")?,
                })
            }
            [scope, action, rest @ ..] if scope == "run" && action == "start" => {
                Ok(Self::StartRun {
                    target: required_client_target(rest)?,
                    session_id: required_flag(rest, "--session")?,
                    input: required_flag(rest, "--input")?,
                })
            }
            [scope, action, rest @ ..] if scope == "run" && action == "status" => {
                Ok(Self::RunStatus {
                    target: required_client_target(rest)?,
                    run_id: required_flag(rest, "--run")?,
                })
            }
            [scope, action, rest @ ..] if scope == "display" && action == "replay" => {
                let after = required_flag(rest, "--after")?;
                Ok(Self::ReplayDisplay {
                    target: required_client_target(rest)?,
                    run_id: required_flag(rest, "--run")?,
                    after_sequence: after
                        .parse()
                        .map_err(|_| CliError::InvalidAfter(after.clone()))?,
                })
            }
            _ => Err(CliError::Usage(
                "usage: session create (--store PATH | --socket PATH) --workspace PATH | run start (--store PATH | --socket PATH) --session ID --input TEXT | run status (--store PATH | --socket PATH) --run ID | display replay (--store PATH | --socket PATH) --run ID --after SEQ"
                    .to_string(),
            )),
        }
    }

    fn execute<R>(self, runtime: R) -> Result<String, CliError>
    where
        R: AgentRuntime + Clone,
    {
        match self {
            Self::CreateSession {
                target,
                workspace_path,
            } => {
                let mut client = target.open(runtime)?;
                let session = client.create_session(RunTarget::local_daemon(workspace_path))?;
                Ok(format!(
                    "session_id={}\ntape_id={}",
                    session.session_id, session.tape_id
                ))
            }
            Self::StartRun {
                target,
                session_id,
                input,
            } => {
                let mut client = target.open(runtime)?;
                let run = client.start_run(&session_id, input)?;
                Ok(format!(
                    "run_id={}\nstatus={}\nresult={}",
                    run.run_id,
                    run_status_name(&run.status),
                    required_run_result(&run.result)
                ))
            }
            Self::RunStatus { target, run_id } => {
                let mut client = target.open(runtime)?;
                let run = client
                    .run(&run_id)?
                    .ok_or_else(|| CliError::Usage(format!("run not found: {run_id}")))?;
                Ok(format_run_status(&run))
            }
            Self::ReplayDisplay {
                target,
                run_id,
                after_sequence,
            } => {
                let mut client = target.open(runtime)?;
                let events = client.replay_display_events(&run_id, after_sequence)?;
                Ok(events
                    .iter()
                    .map(format_display_event)
                    .collect::<Vec<_>>()
                    .join("\n"))
            }
        }
    }
}

enum CliDaemonClient<R> {
    Store(LocalDaemon<R>),
    #[cfg(unix)]
    Socket(LocalDaemonIpcClient),
}

impl ClientTarget {
    fn open<R>(self, runtime: R) -> Result<CliDaemonClient<R>, CliError>
    where
        R: AgentRuntime + Clone,
    {
        match self {
            Self::Store { store_path } => Ok(CliDaemonClient::Store(LocalDaemon::open(
                store_path, runtime,
            )?)),
            Self::Socket { socket_path } => {
                #[cfg(unix)]
                {
                    Ok(CliDaemonClient::Socket(LocalDaemonIpcClient::connect(
                        socket_path,
                    )?))
                }
                #[cfg(not(unix))]
                {
                    let _ = socket_path;
                    Err(CliError::Usage(
                        "--socket is only supported on Unix platforms".to_string(),
                    ))
                }
            }
        }
    }
}

impl<R> CliDaemonClient<R>
where
    R: AgentRuntime + Clone,
{
    fn create_session(
        &mut self,
        default_run_target: RunTarget,
    ) -> Result<crate::session::SessionRecord, DaemonError> {
        match self {
            Self::Store(daemon) => daemon.create_session(default_run_target),
            #[cfg(unix)]
            Self::Socket(client) => client.create_session(default_run_target),
        }
    }

    fn start_run(
        &mut self,
        session_id: &str,
        input: impl Into<String>,
    ) -> Result<crate::session::RunRecord, DaemonError> {
        match self {
            Self::Store(daemon) => daemon.start_run(session_id, input),
            #[cfg(unix)]
            Self::Socket(client) => client.start_run(session_id, input),
        }
    }

    fn replay_display_events(
        &mut self,
        run_id: &str,
        after_sequence: u64,
    ) -> Result<Vec<DisplayEvent>, DaemonError> {
        match self {
            Self::Store(daemon) => daemon.replay_display_events(run_id, after_sequence),
            #[cfg(unix)]
            Self::Socket(client) => client.replay_display_events(run_id, after_sequence),
        }
    }

    fn run(&mut self, run_id: &str) -> Result<Option<RunRecord>, DaemonError> {
        match self {
            Self::Store(daemon) => daemon.run(run_id),
            #[cfg(unix)]
            Self::Socket(client) => client.run(run_id),
        }
    }
}

fn required_client_target(args: &[String]) -> Result<ClientTarget, CliError> {
    let store = optional_flag(args, "--store");
    let socket = optional_flag(args, "--socket");
    match (store, socket) {
        (Some(store_path), None) => Ok(ClientTarget::Store {
            store_path: PathBuf::from(store_path),
        }),
        (None, Some(socket_path)) => Ok(ClientTarget::Socket {
            socket_path: PathBuf::from(socket_path),
        }),
        (Some(_), Some(_)) => Err(CliError::Usage(
            "--store and --socket are mutually exclusive".to_string(),
        )),
        (None, None) => Err(CliError::Usage(
            "missing required client target: --store PATH or --socket PATH".to_string(),
        )),
    }
}

fn required_flag(args: &[String], name: &str) -> Result<String, CliError> {
    optional_flag(args, name)
        .ok_or_else(|| CliError::Usage(format!("missing required flag {name}")))
}

fn optional_flag(args: &[String], name: &str) -> Option<String> {
    args.windows(2).find_map(|window| {
        if window[0] == name {
            Some(window[1].clone())
        } else {
            None
        }
    })
}

fn run_status_name(status: &RunStatus) -> &'static str {
    match status {
        RunStatus::Queued => "queued",
        RunStatus::Running => "running",
        RunStatus::WaitingApproval => "waiting_approval",
        RunStatus::Cancelling => "cancelling",
        RunStatus::Cancelled => "cancelled",
        RunStatus::Completed => "completed",
        RunStatus::Failed => "failed",
        RunStatus::Interrupted => "interrupted",
    }
}

fn format_display_event(event: &DisplayEvent) -> String {
    match event.kind {
        DisplayEventKind::Progress => format!("{} progress", event.sequence),
        DisplayEventKind::AssistantTextDelta => {
            format!(
                "{} assistant_text_delta text={}",
                event.sequence,
                required_string_payload(&event.payload, "text")
            )
        }
        DisplayEventKind::FinalResult => {
            format!(
                "{} final_result result={}",
                event.sequence,
                required_string_payload(&event.payload, "result")
            )
        }
        DisplayEventKind::ToolStatus => format!("{} tool_status", event.sequence),
        DisplayEventKind::ApprovalPrompt => format!("{} approval_prompt", event.sequence),
    }
}

fn required_run_result(result: &Option<String>) -> &str {
    result
        .as_deref()
        .expect("completed CLI run must contain a result")
}

fn format_run_status(run: &RunRecord) -> String {
    let mut lines = vec![
        format!("run_id={}", run.run_id),
        format!("session_id={}", run.session_id),
        format!("status={}", run_status_name(&run.status)),
    ];
    if let Some(result) = &run.result {
        lines.push(format!("result={result}"));
    }
    if let Some(error) = &run.error {
        lines.push(format!("error={error}"));
    }
    lines.join("\n")
}

fn required_string_payload<'a>(payload: &'a Value, field: &str) -> &'a str {
    payload
        .get(field)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("DisplayEvent payload must contain string field {field}"))
}
