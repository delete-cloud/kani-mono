use kani_mono::sandbox::{SandboxError, SandboxedEnvironment};
use kani_mono::session::RunTarget;
use pretty_assertions::assert_eq;
use tempfile::tempdir;

#[test]
fn local_sandbox_resolves_paths_inside_workspace_only() {
    let workspace = tempdir().expect("workspace temp dir");
    let target = RunTarget::local_daemon(workspace.path().to_string_lossy());
    let environment =
        SandboxedEnvironment::from_run_target(&target).expect("local daemon target is sandboxable");

    assert_eq!(
        environment
            .resolve_workspace_path("src/lib.rs")
            .expect("relative path resolves"),
        workspace.path().join("src").join("lib.rs")
    );
    assert_eq!(
        environment
            .resolve_workspace_path("/etc/passwd")
            .expect_err("absolute path is denied"),
        SandboxError::AbsolutePathDenied
    );
    assert_eq!(
        environment
            .resolve_workspace_path("../outside")
            .expect_err("parent traversal is denied"),
        SandboxError::ParentTraversalDenied
    );
}
