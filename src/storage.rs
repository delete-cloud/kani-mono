use std::path::Path;

use crate::session::{ApprovalRecord, CancelIntent, CheckpointRecord, RunRecord, SessionRecord};
use crate::stream::{DisplayEvent, RuntimeEvent};
use rusqlite::{Connection, OptionalExtension, params};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("runtime event not found: {0}")]
    RuntimeEventNotFound(String),
}

pub type StoreResult<T> = Result<T, StoreError>;

pub trait ControlPlaneStore {
    fn save_session(&self, record: &SessionRecord) -> StoreResult<()>;
    fn load_session(&self, session_id: &str) -> StoreResult<Option<SessionRecord>>;
    fn save_run(&self, record: &RunRecord) -> StoreResult<()>;
    fn load_run(&self, run_id: &str) -> StoreResult<Option<RunRecord>>;
    fn save_approval(&self, record: &ApprovalRecord) -> StoreResult<()>;
    fn load_approval(&self, approval_id: &str) -> StoreResult<Option<ApprovalRecord>>;
    fn save_cancel(&self, record: &CancelIntent) -> StoreResult<()>;
    fn load_cancel(&self, cancel_id: &str) -> StoreResult<Option<CancelIntent>>;
    fn save_checkpoint(&self, record: &CheckpointRecord) -> StoreResult<()>;
    fn load_checkpoint(&self, checkpoint_id: &str) -> StoreResult<Option<CheckpointRecord>>;
    fn list_checkpoints_for_run(&self, run_id: &str) -> StoreResult<Vec<CheckpointRecord>>;
}

pub struct SqliteControlPlaneStore {
    connection: Connection,
}

impl SqliteControlPlaneStore {
    pub fn open(path: impl AsRef<Path>) -> StoreResult<Self> {
        let connection = Connection::open(path)?;
        let store = Self { connection };
        store.create_schema()?;
        Ok(store)
    }

    pub fn save_session(&self, record: &SessionRecord) -> StoreResult<()> {
        self.connection.execute(
            "
            INSERT INTO sessions (session_id, record_json)
            VALUES (?1, ?2)
            ON CONFLICT(session_id) DO UPDATE SET record_json = excluded.record_json
            ",
            params![record.session_id, to_json(record)?],
        )?;
        Ok(())
    }

    pub fn load_session(&self, session_id: &str) -> StoreResult<Option<SessionRecord>> {
        self.load_record(
            "SELECT record_json FROM sessions WHERE session_id = ?1",
            session_id,
        )
    }

    pub fn save_run(&self, record: &RunRecord) -> StoreResult<()> {
        self.connection.execute(
            "
            INSERT INTO runs (run_id, session_id, record_json)
            VALUES (?1, ?2, ?3)
            ON CONFLICT(run_id) DO UPDATE SET
                session_id = excluded.session_id,
                record_json = excluded.record_json
            ",
            params![record.run_id, record.session_id, to_json(record)?],
        )?;
        Ok(())
    }

    pub fn load_run(&self, run_id: &str) -> StoreResult<Option<RunRecord>> {
        self.load_record("SELECT record_json FROM runs WHERE run_id = ?1", run_id)
    }

    pub fn save_approval(&self, record: &ApprovalRecord) -> StoreResult<()> {
        self.connection.execute(
            "
            INSERT INTO approvals (approval_id, run_id, record_json)
            VALUES (?1, ?2, ?3)
            ON CONFLICT(approval_id) DO UPDATE SET
                run_id = excluded.run_id,
                record_json = excluded.record_json
            ",
            params![record.approval_id, record.run_id, to_json(record)?],
        )?;
        Ok(())
    }

    pub fn load_approval(&self, approval_id: &str) -> StoreResult<Option<ApprovalRecord>> {
        self.load_record(
            "SELECT record_json FROM approvals WHERE approval_id = ?1",
            approval_id,
        )
    }

    pub fn save_cancel(&self, record: &CancelIntent) -> StoreResult<()> {
        self.connection.execute(
            "
            INSERT INTO cancels (cancel_id, run_id, record_json)
            VALUES (?1, ?2, ?3)
            ON CONFLICT(cancel_id) DO UPDATE SET
                run_id = excluded.run_id,
                record_json = excluded.record_json
            ",
            params![record.cancel_id, record.run_id, to_json(record)?],
        )?;
        Ok(())
    }

    pub fn load_cancel(&self, cancel_id: &str) -> StoreResult<Option<CancelIntent>> {
        self.load_record(
            "SELECT record_json FROM cancels WHERE cancel_id = ?1",
            cancel_id,
        )
    }

    pub fn append_runtime_event(&self, event: RuntimeEvent) -> StoreResult<RuntimeEvent> {
        self.connection.execute(
            "
            INSERT OR IGNORE INTO runtime_events (event_id, run_id, record_json)
            VALUES (?1, ?2, ?3)
            ",
            params![event.event_id, event.run_id, to_json(&event)?],
        )?;
        self.load_runtime_event(&event.event_id)?
            .ok_or_else(|| StoreError::Sqlite(rusqlite::Error::QueryReturnedNoRows))
    }

    pub fn replay_runtime_events(
        &self,
        run_id: &str,
        after_sequence: u64,
    ) -> StoreResult<Vec<RuntimeEvent>> {
        let mut statement = self.connection.prepare(
            "
            SELECT sequence, record_json
            FROM runtime_events
            WHERE run_id = ?1 AND sequence > ?2
            ORDER BY sequence
            ",
        )?;
        let rows = statement.query_map(params![run_id, after_sequence], |row| {
            runtime_event_from_row(row.get(0)?, row.get(1)?)
                .map_err(|err| rusqlite::Error::ToSqlConversionFailure(Box::new(err)))
        })?;

        let mut events = Vec::new();
        for row in rows {
            events.push(row?);
        }
        Ok(events)
    }

    pub fn project_display_event(&self, event: &RuntimeEvent) -> StoreResult<DisplayEvent> {
        let source = self
            .load_runtime_event(&event.event_id)?
            .ok_or_else(|| StoreError::RuntimeEventNotFound(event.event_id.clone()))?;
        let display = DisplayEvent::project_from_runtime(&source, 0);
        self.connection.execute(
            "
            INSERT OR IGNORE INTO display_events (
                source_runtime_event_id,
                run_id,
                record_json
            )
            VALUES (?1, ?2, ?3)
            ",
            params![source.event_id, source.run_id, to_json(&display)?],
        )?;
        self.load_display_event(&source.event_id)?
            .ok_or_else(|| StoreError::Sqlite(rusqlite::Error::QueryReturnedNoRows))
    }

    pub fn replay_display_events(
        &self,
        run_id: &str,
        after_sequence: u64,
    ) -> StoreResult<Vec<DisplayEvent>> {
        let mut statement = self.connection.prepare(
            "
            SELECT sequence, record_json
            FROM display_events
            WHERE run_id = ?1 AND sequence > ?2
            ORDER BY sequence
            ",
        )?;
        let rows = statement.query_map(params![run_id, after_sequence], |row| {
            display_event_from_row(row.get(0)?, row.get(1)?)
                .map_err(|err| rusqlite::Error::ToSqlConversionFailure(Box::new(err)))
        })?;

        let mut events = Vec::new();
        for row in rows {
            events.push(row?);
        }
        Ok(events)
    }

    pub fn save_checkpoint(&self, record: &CheckpointRecord) -> StoreResult<()> {
        self.connection.execute(
            "
            INSERT INTO checkpoints (checkpoint_id, run_id, record_json)
            VALUES (?1, ?2, ?3)
            ON CONFLICT(checkpoint_id) DO UPDATE SET
                run_id = excluded.run_id,
                record_json = excluded.record_json
            ",
            params![record.checkpoint_id, record.run_id, to_json(record)?],
        )?;
        Ok(())
    }

    pub fn load_checkpoint(&self, checkpoint_id: &str) -> StoreResult<Option<CheckpointRecord>> {
        self.load_record(
            "SELECT record_json FROM checkpoints WHERE checkpoint_id = ?1",
            checkpoint_id,
        )
    }

    pub fn list_checkpoints_for_run(&self, run_id: &str) -> StoreResult<Vec<CheckpointRecord>> {
        let mut statement = self.connection.prepare(
            "
            SELECT record_json
            FROM checkpoints
            WHERE run_id = ?1
            ORDER BY sequence
            ",
        )?;
        let rows = statement.query_map(params![run_id], |row| {
            let payload: String = row.get(0)?;
            serde_json::from_str(&payload)
                .map_err(|err| rusqlite::Error::ToSqlConversionFailure(Box::new(err)))
        })?;

        let mut checkpoints = Vec::new();
        for row in rows {
            checkpoints.push(row?);
        }
        Ok(checkpoints)
    }

    fn load_runtime_event(&self, event_id: &str) -> StoreResult<Option<RuntimeEvent>> {
        let row = self
            .connection
            .query_row(
                "SELECT sequence, record_json FROM runtime_events WHERE event_id = ?1",
                params![event_id],
                |row| {
                    runtime_event_from_row(row.get(0)?, row.get(1)?)
                        .map_err(|err| rusqlite::Error::ToSqlConversionFailure(Box::new(err)))
                },
            )
            .optional()?;
        Ok(row)
    }

    fn load_display_event(
        &self,
        source_runtime_event_id: &str,
    ) -> StoreResult<Option<DisplayEvent>> {
        let row = self
            .connection
            .query_row(
                "
                SELECT sequence, record_json
                FROM display_events
                WHERE source_runtime_event_id = ?1
                ",
                params![source_runtime_event_id],
                |row| {
                    display_event_from_row(row.get(0)?, row.get(1)?)
                        .map_err(|err| rusqlite::Error::ToSqlConversionFailure(Box::new(err)))
                },
            )
            .optional()?;
        Ok(row)
    }

    fn load_record<T>(&self, query: &str, id: &str) -> StoreResult<Option<T>>
    where
        T: serde::de::DeserializeOwned,
    {
        let payload = self
            .connection
            .query_row(query, params![id], |row| row.get::<_, String>(0))
            .optional()?;
        payload
            .map(|value| serde_json::from_str(&value).map_err(StoreError::from))
            .transpose()
    }

    fn create_schema(&self) -> StoreResult<()> {
        self.connection.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS sessions (
                session_id TEXT PRIMARY KEY,
                record_json TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS runs (
                run_id TEXT PRIMARY KEY,
                session_id TEXT NOT NULL,
                record_json TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS runs_session_id_idx ON runs (session_id, run_id);

            CREATE TABLE IF NOT EXISTS approvals (
                approval_id TEXT PRIMARY KEY,
                run_id TEXT NOT NULL,
                record_json TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS approvals_run_id_idx ON approvals (run_id, approval_id);

            CREATE TABLE IF NOT EXISTS cancels (
                cancel_id TEXT PRIMARY KEY,
                run_id TEXT NOT NULL,
                record_json TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS cancels_run_id_idx ON cancels (run_id, cancel_id);

            CREATE TABLE IF NOT EXISTS runtime_events (
                sequence INTEGER PRIMARY KEY AUTOINCREMENT,
                event_id TEXT UNIQUE NOT NULL,
                run_id TEXT NOT NULL,
                record_json TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS runtime_events_run_id_sequence_idx
                ON runtime_events (run_id, sequence);

            CREATE TABLE IF NOT EXISTS display_events (
                sequence INTEGER PRIMARY KEY AUTOINCREMENT,
                source_runtime_event_id TEXT UNIQUE NOT NULL,
                run_id TEXT NOT NULL,
                record_json TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS display_events_run_id_sequence_idx
                ON display_events (run_id, sequence);

            CREATE TABLE IF NOT EXISTS checkpoints (
                sequence INTEGER PRIMARY KEY AUTOINCREMENT,
                checkpoint_id TEXT UNIQUE NOT NULL,
                run_id TEXT NOT NULL,
                record_json TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS checkpoints_run_id_sequence_idx
                ON checkpoints (run_id, sequence);
            ",
        )?;
        Ok(())
    }
}

impl ControlPlaneStore for SqliteControlPlaneStore {
    fn save_session(&self, record: &SessionRecord) -> StoreResult<()> {
        SqliteControlPlaneStore::save_session(self, record)
    }

    fn load_session(&self, session_id: &str) -> StoreResult<Option<SessionRecord>> {
        SqliteControlPlaneStore::load_session(self, session_id)
    }

    fn save_run(&self, record: &RunRecord) -> StoreResult<()> {
        SqliteControlPlaneStore::save_run(self, record)
    }

    fn load_run(&self, run_id: &str) -> StoreResult<Option<RunRecord>> {
        SqliteControlPlaneStore::load_run(self, run_id)
    }

    fn save_approval(&self, record: &ApprovalRecord) -> StoreResult<()> {
        SqliteControlPlaneStore::save_approval(self, record)
    }

    fn load_approval(&self, approval_id: &str) -> StoreResult<Option<ApprovalRecord>> {
        SqliteControlPlaneStore::load_approval(self, approval_id)
    }

    fn save_cancel(&self, record: &CancelIntent) -> StoreResult<()> {
        SqliteControlPlaneStore::save_cancel(self, record)
    }

    fn load_cancel(&self, cancel_id: &str) -> StoreResult<Option<CancelIntent>> {
        SqliteControlPlaneStore::load_cancel(self, cancel_id)
    }

    fn save_checkpoint(&self, record: &CheckpointRecord) -> StoreResult<()> {
        SqliteControlPlaneStore::save_checkpoint(self, record)
    }

    fn load_checkpoint(&self, checkpoint_id: &str) -> StoreResult<Option<CheckpointRecord>> {
        SqliteControlPlaneStore::load_checkpoint(self, checkpoint_id)
    }

    fn list_checkpoints_for_run(&self, run_id: &str) -> StoreResult<Vec<CheckpointRecord>> {
        SqliteControlPlaneStore::list_checkpoints_for_run(self, run_id)
    }
}

fn to_json<T>(record: &T) -> Result<String, serde_json::Error>
where
    T: serde::Serialize,
{
    serde_json::to_string(record)
}

fn runtime_event_from_row(
    sequence: u64,
    record_json: String,
) -> Result<RuntimeEvent, serde_json::Error> {
    let mut event: RuntimeEvent = serde_json::from_str(&record_json)?;
    event.sequence = sequence;
    Ok(event)
}

fn display_event_from_row(
    sequence: u64,
    record_json: String,
) -> Result<DisplayEvent, serde_json::Error> {
    let mut event: DisplayEvent = serde_json::from_str(&record_json)?;
    event.sequence = sequence;
    Ok(event)
}
