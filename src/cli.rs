use std::path::PathBuf;

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
        store_path: PathBuf,
        workspace_path: String,
    },
    StartRun {
        store_path: PathBuf,
        session_id: String,
        input: String,
    },
    ReplayDisplay {
        store_path: PathBuf,
        run_id: String,
        after_sequence: u64,
    },
}

impl CliCommand {
    fn parse(args: &[String]) -> Result<Self, CliError> {
        match args {
            [scope, action, rest @ ..] if scope == "session" && action == "create" => {
                Ok(Self::CreateSession {
                    store_path: PathBuf::from(required_flag(rest, "--store")?),
                    workspace_path: required_flag(rest, "--workspace")?,
                })
            }
            [scope, action, rest @ ..] if scope == "run" && action == "start" => {
                Ok(Self::StartRun {
                    store_path: PathBuf::from(required_flag(rest, "--store")?),
                    session_id: required_flag(rest, "--session")?,
                    input: required_flag(rest, "--input")?,
                })
            }
            [scope, action, rest @ ..] if scope == "display" && action == "replay" => {
                let after = required_flag(rest, "--after")?;
                Ok(Self::ReplayDisplay {
                    store_path: PathBuf::from(required_flag(rest, "--store")?),
                    run_id: required_flag(rest, "--run")?,
                    after_sequence: after
                        .parse()
                        .map_err(|_| CliError::InvalidAfter(after.clone()))?,
                })
            }
            _ => Err(CliError::Usage(
                "usage: session create --store PATH --workspace PATH | run start --store PATH --session ID --input TEXT | display replay --store PATH --run ID --after SEQ"
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
                store_path,
                workspace_path,
            } => {
                let mut daemon = LocalDaemon::open(store_path, runtime)?;
                let session = daemon.create_session(RunTarget::local_daemon(workspace_path))?;
                Ok(format!(
                    "session_id={}\ntape_id={}",
                    session.session_id, session.tape_id
                ))
            }
            Self::StartRun {
                store_path,
                session_id,
                input,
            } => {
                let mut daemon = LocalDaemon::open(store_path, runtime)?;
                let run = daemon.start_run(&session_id, input)?;
                Ok(format!(
                    "run_id={}\nstatus={}\nresult={}",
                    run.run_id,
                    run_status_name(&run.status),
                    required_run_result(&run.result)
                ))
            }
            Self::ReplayDisplay {
                store_path,
                run_id,
                after_sequence,
            } => {
                let daemon = LocalDaemon::open(store_path, runtime)?;
                let events = daemon.replay_display_events(&run_id, after_sequence)?;
                Ok(events
                    .iter()
                    .map(format_display_event)
                    .collect::<Vec<_>>()
                    .join("\n"))
            }
        }
    }
}

fn required_flag(args: &[String], name: &str) -> Result<String, CliError> {
    args.windows(2)
        .find_map(|window| {
            if window[0] == name {
                Some(window[1].clone())
            } else {
                None
            }
        })
        .ok_or_else(|| CliError::Usage(format!("missing required flag {name}")))
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
