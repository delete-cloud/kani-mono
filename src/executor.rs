use serde_json::json;

use crate::runtime::AgentRuntime;
use crate::session::RunRecord;
use crate::stream::{EventLog, RuntimeEvent, RuntimeEventKind};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunExecutionOutcome {
    pub final_text: String,
}

pub trait Executor {
    fn execute(&self, run: &RunRecord, events: &mut EventLog) -> RunExecutionOutcome;
}

pub struct LocalDaemonExecutor<R> {
    runtime: R,
}

impl<R> LocalDaemonExecutor<R>
where
    R: AgentRuntime,
{
    pub fn new(runtime: R) -> Self {
        Self { runtime }
    }
}

impl<R> Executor for LocalDaemonExecutor<R>
where
    R: AgentRuntime,
{
    fn execute(&self, run: &RunRecord, events: &mut EventLog) -> RunExecutionOutcome {
        let started = events.append_runtime(RuntimeEvent::new(
            format!("{}:run-started", run.run_id),
            run.run_id.clone(),
            RuntimeEventKind::RunStarted,
            json!({"input": run.input}),
        ));
        events.project_runtime_event(&started);

        let output = self.runtime.run(&run.input);
        let model_delta = events.append_runtime(RuntimeEvent::new(
            format!("{}:model-delta:0", run.run_id),
            run.run_id.clone(),
            RuntimeEventKind::ModelDelta,
            json!({"text": output.final_text}),
        ));
        events.project_runtime_event(&model_delta);

        let completed = events.append_runtime(RuntimeEvent::new(
            format!("{}:run-completed", run.run_id),
            run.run_id.clone(),
            RuntimeEventKind::RunCompleted,
            json!({"result": output.final_text}),
        ));
        events.project_runtime_event(&completed);

        RunExecutionOutcome {
            final_text: output.final_text,
        }
    }
}
