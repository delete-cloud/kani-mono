use std::path::{Component, Path, PathBuf};

use crate::session::{RunTarget, WorkspaceRef};
use thiserror::Error;

#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum SandboxError {
    #[error("only workspace_scoped filesystem isolation is supported")]
    UnsupportedFilesystemPolicy,
    #[error("absolute workspace paths are denied")]
    AbsolutePathDenied,
    #[error("parent traversal outside the workspace is denied")]
    ParentTraversalDenied,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SandboxedEnvironment {
    workspace_root: PathBuf,
}

impl SandboxedEnvironment {
    pub fn from_run_target(target: &RunTarget) -> Result<Self, SandboxError> {
        if target.isolation.filesystem != "workspace_scoped" {
            return Err(SandboxError::UnsupportedFilesystemPolicy);
        }

        match &target.workspace {
            WorkspaceRef::LocalPath { path } => Ok(Self {
                workspace_root: PathBuf::from(path),
            }),
        }
    }

    pub fn workspace_root(&self) -> &Path {
        &self.workspace_root
    }

    pub fn resolve_workspace_path(&self, path: impl AsRef<Path>) -> Result<PathBuf, SandboxError> {
        let mut relative = PathBuf::new();
        for component in path.as_ref().components() {
            match component {
                Component::Normal(segment) => relative.push(segment),
                Component::CurDir => {}
                Component::ParentDir => return Err(SandboxError::ParentTraversalDenied),
                Component::RootDir | Component::Prefix(_) => {
                    return Err(SandboxError::AbsolutePathDenied);
                }
            }
        }

        Ok(self.workspace_root.join(relative))
    }
}
