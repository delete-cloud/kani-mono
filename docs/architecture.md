# Kani Mono Architecture

## Product Definition

Kani Mono is a Rust rewrite of Coding-Agent-Loop. Its target shape is:

```text
run-centric durable coding agent runtime
```

It is not a CLI patch tool, a prompt-to-patch script, or an HTTP wrapper around a
runtime.

The canonical chain is:

```text
Client
  -> ControlPlane / SessionService
  -> RunCoordinator
  -> Executor
  -> Runtime
  -> SandboxedEnvironment
  -> Workspace
```

## Source Of Truth

Tape is the authoritative append-only log. It is the only durable source of
truth for model-visible conversation facts, tool calls/results, anchors,
handoffs, restore markers, topic boundaries, and resume boundaries.

Derived views and coordination records include:

```text
SessionRecord
RunRecord
RuntimeEvent
DisplayEvent
TopicRange
Memory / ContextPack
Metrics
Debug index
Checkpoint snapshot
KV-cache
```

These records may be persisted and indexed, but they must not become required
inputs for replaying the authoritative tape.

## Tape Entries

Tape entries are append-only:

```text
TapeEntry
  seq
  entry_id
  kind
  payload
  meta
  run_id
  branch_id / epoch
  active_state
```

The active view is a projection over tape, not a mutation of tape.

## Session And Run

Session is a long-lived container. Run is one execution attempt.

Session stores durable defaults and context references:

```text
session_id
default_run_target
current_run_id
tape_id
status
metadata
```

Run is the scheduling and execution unit:

```text
run_id
session_id
target
status
input
result
error
resume_from
checkpoint_refs
trace_refs
```

Resume means: create a new run from durable session context. It is not task
retry and not old-process restoration.

## Checkpoint And Restore

Checkpoint is a recovery reference and snapshot evidence. It is not source of
truth.

Restore must not physically truncate tape. Restore is:

```text
append restore marker + switch active view
```

Restore marker:

```text
RestoreMarker
  checkpoint_id
  restore_to_seq
  previous_head_seq
  restore_reason
  new_epoch / branch_id
```

After restore:

- Old entries remain in tape.
- Entries after the checkpoint in the previous active head are marked
  `superseded` / `inactive` for the current active view.
- The active tape view returns to the checkpoint prefix plus the restore marker.
- Later runs append under the new branch or epoch.

KV-cache keys must include at least:

```text
tape_id
branch_id / tape_epoch
visible_head_seq
context_digest
model config
```

This makes restore invalidate stale cache entries without deleting history.

## Runtime Events And Display Events

RuntimeEvent is an internal fact stream for replay, audit, debug, and resume:

```text
run_started
model_delta
tool_call_started
approval_requested
checkpoint_created
run_completed
```

DisplayEvent is the user-facing projection consumed by CLI/TUI/Web clients:

```text
assistant_text_delta
tool_status
approval_prompt
progress
final_result
```

Clients consume DisplayEvent. RuntimeEvent remains internal.

## Approval And Cancel

Approval is a durable interaction:

```text
ApprovalRecord
  approval_id
  run_id
  tool_call_ref
  status
  request_payload
  decision
  resolved_at
```

Cancel is a durable intent:

```text
cancel_requested -> run.cancelling -> executor observes -> runtime cancelled -> run.cancelled
```

Neither approval nor cancel may exist only in memory.

## RunTarget

RunTarget is the canonical scheduling model:

```text
RunTarget =
  WorkspaceRef
  + ExecutorRef
  + IsolationPolicy
  + RunConstraints
```

ExecutionBinding is deprecated compatibility input. New code should use
RunTarget only.

## Module Boundaries

```text
core
  ids, errors, metadata, JSON envelopes

tape
  append-only entries, anchors, active view, restore marker, indexes

session
  SessionRecord, RunRecord, ResumeSnapshot, stores

stream
  RuntimeEvent, DisplayEvent, replay cursor, SSE/terminal projection

controlplane
  SessionService, RunCoordinator, ApprovalService, CancelService

executor
  LocalDaemonExecutor, future ManagedPoolExecutor / AttachedExecutor

runtime
  pure agent loop, model loop, tool loop, checkpoint emission

model
  provider-neutral messages, stream events, replay provider

tools
  schema, registry, execution, approval metadata, MCP contracts

environment
  WorkspaceRef, WorkspaceProvider, shell/filesystem abstraction

sandbox
  filesystem/shell/network/secret policy wrapper

daemon
  local durable process wiring

server
  cloud/server API wiring

cli
  client only

testkit
  inline executor, fake model, in-memory store
```

## MVP Scope

The first product path is Local Daemon Mode:

```text
CLI / TUI / REPL
  -> Local daemon ControlPlane
  -> RunCoordinator
  -> LocalDaemonExecutor
  -> Runtime
  -> SandboxedEnvironment
  -> Local Workspace
```

MVP includes:

```text
SQLite store
append-only tape
restore marker
RunTarget
durable session/run
runtime/display event replay
durable approval
durable cancel
basic checkpoint snapshot
resume creates new run
default sandbox
```

MVP excludes:

```text
cloud managed executor
attached executor
multi-tenant auth
workspace sync
worker lease/fencing beyond local
team dashboard
complex memory/RAG
```

## Current Implementation Slice

The current Rust codebase has the first architecture contracts in place:

- `tape` models append-only entries, restore markers, active views, superseded
  entries, and KV-cache identity across restore epochs.
- `session` models SessionRecord, RunRecord, RunTarget, durable approval, cancel,
  and resume-as-new-run semantics.
- `storage` provides SQLite-backed focused store contracts for sessions, runs,
  approvals/cancels, checkpoints, runtime event replay, and durable DisplayEvent
  projection. `ControlPlaneStore` remains a compatibility composition of those
  narrower contracts.
- `controlplane` exposes an in-memory SessionService for tests and a
  DurableSessionService backed by the store contract.
- `executor` and `runtime` contain the first LocalDaemonExecutor and replay
  runtime seam used by RunCoordinator contract tests. DurableRunCoordinator can
  persist executor-emitted RuntimeEvent and DisplayEvent records through the
  control-plane store.
- `daemon` exposes the first LocalDaemon facade for local create-session,
  start-run, and display-event replay over SQLite. It also has an in-process
  LocalDaemonProcess actor with explicit stop semantics, so clients can be
  dropped and recreated without ending the daemon. Unix platforms also have the
  first local socket IPC server/client for create-session, start-run, and
  display-event replay. It is not yet an HTTP server.
- `cli` exposes the first client command boundary and binary entrypoint for
  creating local sessions, starting runs, and replaying DisplayEvent records.
  It supports direct SQLite-backed `--store` mode and Unix socket-backed
  `--socket` mode. The binary still uses the replay runtime seam for direct
  local runs; real model provider integration remains future work.
- `sandbox` implements the first workspace-scoped filesystem boundary for local
  paths.

This is not a complete MVP yet. The remaining MVP work includes CLI-managed
daemon lifecycle commands, richer CLI client commands, real provider/tool
runtime integration, shell execution through SandboxedEnvironment, and
end-to-end resume from durable context.

## Completion Definition

The rewrite is not complete until these requirements are implemented and
verified:

- Client can create a session and start a run through the local daemon path.
- ControlPlane persists session, run, approval, cancel, checkpoint, runtime
  event, and display event data.
- RunCoordinator schedules by RunTarget.
- LocalDaemonExecutor owns runtime execution.
- Runtime does not depend on CLI, HTTP, daemon, database, or deployment mode.
- Tape is append-only under forward execution and restore.
- Restore appends a restore marker and switches active view without truncating
  tape.
- Resume creates a new run from durable session context.
- RuntimeEvent and DisplayEvent are separate and replayable by cursor.
- Approval and cancel are durable intents.
- SandboxedEnvironment wraps workspace file and shell access.
- The MVP test suite passes with `cargo test --all`.
