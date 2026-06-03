# AGENTS.md

## Scope

Applies to the whole repository.

## Project Shape

This repository is a Rust rewrite of Coding-Agent-Loop. The target product is a
run-centric durable coding agent runtime, not a single-shot CLI agent.

Canonical architecture:

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

- `Tape` is the authoritative append-only log.
- Session, run, stream, topic, memory, metrics, debug indexes, checkpoints, and
  KV-cache are derived views or coordination records over tape.
- Restore must not physically truncate tape. Restore appends a marker, creates a
  new active branch/epoch, and marks later entries from the old active view as
  superseded for that view.

## First Product Path

The first implementation target is Local Daemon Mode:

```text
CLI / TUI / REPL
  -> Local daemon ControlPlane
  -> RunCoordinator
  -> LocalDaemonExecutor
  -> Runtime
  -> SandboxedEnvironment
  -> Local Workspace
```

Do not implement cloud managed execution, attached executors, multi-tenant auth,
workspace sync, team dashboards, or complex memory/RAG in the MVP.

## Module Ownership

- `core`: ids, errors, metadata, JSON envelopes.
- `tape`: append-only entries, anchors, active views, restore markers, indexes.
- `session`: session records, run records, resume snapshots, stores.
- `stream`: runtime events, display events, replay cursors, projections.
- `controlplane`: session service, run coordinator, approval and cancel services.
- `executor`: local daemon executor and future executor traits.
- `runtime`: pure agent loop, model loop, tool loop, checkpoint emission.
- `model`: provider-neutral model messages and replay providers.
- `tools`: schemas, registry, execution, approval metadata, MCP contracts.
- `environment`: workspace refs, workspace providers, filesystem/shell boundary.
- `sandbox`: filesystem, shell, network, and secret policy wrapper.
- `daemon`, `server`, `cli`: product wiring only.
- `testkit`: inline executor, fake model, in-memory stores.

## Working Rules

- Use Rust stable tooling through `cargo`.
- Write tests before implementation for new behavior.
- Prefer small, explicit modules over a new god object.
- Do not let `SessionRecord` store runtime handles, async tasks, provider
  instances, queues, adapters, or in-memory approval waiters.
- Do not let `Runtime` depend on CLI, HTTP, daemon, database, or deployment mode.
- Do not let clients execute the agent loop directly.
- Keep durable approval and cancel as store-backed intents.
- Use `ExecutionBinding` only as deprecated compatibility input if introduced;
  canonical scheduling uses `RunTarget`.

## Verification

Before claiming completion, run the relevant commands:

```bash
cargo test --all
cargo fmt --all --check
cargo clippy --all-targets --all-features -- -D warnings
```

For architecture-only edits, run at least:

```bash
cargo test --all
```

if a Cargo project exists.

