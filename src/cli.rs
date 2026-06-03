use std::path::PathBuf;

#[cfg(unix)]
use crate::daemon::LocalDaemonIpcClient;
use crate::daemon::{DaemonError, LocalDaemon};
use crate::runtime::AgentRuntime;
use crate::session::{RunStatus, RunTarget};
use crate::stream::{DisplayEvent, DisplayEventKind};
use serde_json::Value;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum CliError {
    #[error("{0}")]
    Usage(String),
    #[error("invalid --after value: {0}")]
    InvalidAfter(String),
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
                "usage: session create (--store PATH | --socket PATH) --workspace PATH | run start (--store PATH | --socket PATH) --session ID --input TEXT | display replay (--store PATH | --socket PATH) --run ID --after SEQ"
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

fn required_string_payload<'a>(payload: &'a Value, field: &str) -> &'a str {
    payload
        .get(field)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("DisplayEvent payload must contain string field {field}"))
}
