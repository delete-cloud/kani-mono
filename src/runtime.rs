#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RuntimeOutput {
    pub final_text: String,
}

pub trait AgentRuntime {
    fn run(&self, input: &str) -> RuntimeOutput;
}

#[derive(Clone, Debug)]
pub struct ReplayRuntime {
    final_text: String,
}

impl ReplayRuntime {
    pub fn new(final_text: impl Into<String>) -> Self {
        Self {
            final_text: final_text.into(),
        }
    }
}

impl AgentRuntime for ReplayRuntime {
    fn run(&self, _input: &str) -> RuntimeOutput {
        RuntimeOutput {
            final_text: self.final_text.clone(),
        }
    }
}
