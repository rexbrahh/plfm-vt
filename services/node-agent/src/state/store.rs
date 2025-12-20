//! SQLite-based state store for node agent.
//!
//! This provides durable storage for node and instance state,
//! enabling recovery after agent restarts.

use std::path::Path;

use rusqlite::{params, Connection, OptionalExtension};
use thiserror::Error;
use tracing::debug;

/// Errors from state store operations.
#[derive(Debug, Error)]
pub enum StateStoreError {
    #[error("SQLite error: {0}")]
    Sqlite(#[from] rusqlite::Error),

    #[error("State not found: {0}")]
    NotFound(String),

    #[error("Invalid state: {0}")]
    Invalid(String),
}

/// Instance lifecycle phase.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstancePhase {
    /// Instance is being created.
    Creating,
    /// Instance is starting.
    Starting,
    /// Instance is running.
    Running,
    /// Instance is stopping.
    Stopping,
    /// Instance has stopped.
    Stopped,
    /// Instance has failed.
    Failed,
}

impl InstancePhase {
    fn as_str(&self) -> &'static str {
        match self {
            Self::Creating => "creating",
            Self::Starting => "starting",
            Self::Running => "running",
            Self::Stopping => "stopping",
            Self::Stopped => "stopped",
            Self::Failed => "failed",
        }
    }

    fn from_str(s: &str) -> Option<Self> {
        match s {
            "creating" => Some(Self::Creating),
            "starting" => Some(Self::Starting),
            "running" => Some(Self::Running),
            "stopping" => Some(Self::Stopping),
            "stopped" => Some(Self::Stopped),
            "failed" => Some(Self::Failed),
            _ => None,
        }
    }
}

/// Node-level state.
#[derive(Debug, Clone, Default)]
pub struct NodeState {
    /// Current plan version.
    pub plan_version: i64,
    /// Event cursor for resuming sync.
    pub event_cursor: Option<String>,
    /// Last heartbeat timestamp (Unix seconds).
    pub last_heartbeat: i64,
}

/// Instance record in the state store.
#[derive(Debug, Clone)]
pub struct InstanceRecord {
    /// Instance ID.
    pub instance_id: String,
    /// Current phase.
    pub phase: InstancePhase,
    /// Spec revision.
    pub spec_revision: i64,
    /// Boot ID (for detecting restarts).
    pub boot_id: String,
    /// API socket path.
    pub socket_path: Option<String>,
    /// Root disk digest.
    pub rootdisk_digest: Option<String>,
    /// Created timestamp (Unix seconds).
    pub created_at: i64,
    /// Updated timestamp (Unix seconds).
    pub updated_at: i64,
}

/// SQLite state store.
pub struct StateStore {
    conn: Connection,
}

impl StateStore {
    /// Open or create a state store at the given path.
    pub fn open<P: AsRef<Path>>(path: P) -> Result<Self, StateStoreError> {
        let conn = Connection::open(path)?;

        // Enable WAL mode for better concurrency
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;")?;

        let store = Self { conn };
        store.init_schema()?;

        Ok(store)
    }

    /// Open an in-memory store (for testing).
    pub fn open_in_memory() -> Result<Self, StateStoreError> {
        let conn = Connection::open_in_memory()?;
        let store = Self { conn };
        store.init_schema()?;
        Ok(store)
    }

    /// Initialize database schema.
    fn init_schema(&self) -> Result<(), StateStoreError> {
        self.conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS node_state (
                id INTEGER PRIMARY KEY CHECK (id = 1),
                plan_version INTEGER NOT NULL DEFAULT 0,
                event_cursor TEXT,
                last_heartbeat INTEGER NOT NULL DEFAULT 0
            );

            INSERT OR IGNORE INTO node_state (id) VALUES (1);

            CREATE TABLE IF NOT EXISTS instances (
                instance_id TEXT PRIMARY KEY,
                phase TEXT NOT NULL,
                spec_revision INTEGER NOT NULL DEFAULT 0,
                boot_id TEXT NOT NULL,
                socket_path TEXT,
                rootdisk_digest TEXT,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL
            );

            CREATE INDEX IF NOT EXISTS idx_instances_phase ON instances(phase);
            "#,
        )?;

        debug!("State store schema initialized");
        Ok(())
    }

    /// Get node state.
    pub fn get_node_state(&self) -> Result<NodeState, StateStoreError> {
        let mut stmt = self.conn.prepare(
            "SELECT plan_version, event_cursor, last_heartbeat FROM node_state WHERE id = 1",
        )?;

        stmt.query_row([], |row| {
            Ok(NodeState {
                plan_version: row.get(0)?,
                event_cursor: row.get(1)?,
                last_heartbeat: row.get(2)?,
            })
        })
        .map_err(Into::into)
    }

    /// Update node state.
    pub fn set_node_state(&self, state: &NodeState) -> Result<(), StateStoreError> {
        self.conn.execute(
            "UPDATE node_state SET plan_version = ?1, event_cursor = ?2, last_heartbeat = ?3 WHERE id = 1",
            params![state.plan_version, state.event_cursor, state.last_heartbeat],
        )?;
        Ok(())
    }

    /// Update plan version.
    pub fn set_plan_version(&self, version: i64) -> Result<(), StateStoreError> {
        self.conn.execute(
            "UPDATE node_state SET plan_version = ?1 WHERE id = 1",
            params![version],
        )?;
        Ok(())
    }

    /// Update event cursor.
    pub fn set_event_cursor(&self, cursor: &str) -> Result<(), StateStoreError> {
        self.conn.execute(
            "UPDATE node_state SET event_cursor = ?1 WHERE id = 1",
            params![cursor],
        )?;
        Ok(())
    }

    /// Get an instance record.
    pub fn get_instance(
        &self,
        instance_id: &str,
    ) -> Result<Option<InstanceRecord>, StateStoreError> {
        let mut stmt = self.conn.prepare(
            "SELECT instance_id, phase, spec_revision, boot_id, socket_path, rootdisk_digest, created_at, updated_at
             FROM instances WHERE instance_id = ?1",
        )?;

        stmt.query_row(params![instance_id], |row| {
            let phase_str: String = row.get(1)?;
            let phase = InstancePhase::from_str(&phase_str).unwrap_or(InstancePhase::Failed);

            Ok(InstanceRecord {
                instance_id: row.get(0)?,
                phase,
                spec_revision: row.get(2)?,
                boot_id: row.get(3)?,
                socket_path: row.get(4)?,
                rootdisk_digest: row.get(5)?,
                created_at: row.get(6)?,
                updated_at: row.get(7)?,
            })
        })
        .optional()
        .map_err(Into::into)
    }

    /// Insert or update an instance record.
    pub fn upsert_instance(&self, record: &InstanceRecord) -> Result<(), StateStoreError> {
        self.conn.execute(
            r#"
            INSERT INTO instances (instance_id, phase, spec_revision, boot_id, socket_path, rootdisk_digest, created_at, updated_at)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
            ON CONFLICT(instance_id) DO UPDATE SET
                phase = excluded.phase,
                spec_revision = excluded.spec_revision,
                boot_id = excluded.boot_id,
                socket_path = excluded.socket_path,
                rootdisk_digest = excluded.rootdisk_digest,
                updated_at = excluded.updated_at
            "#,
            params![
                record.instance_id,
                record.phase.as_str(),
                record.spec_revision,
                record.boot_id,
                record.socket_path,
                record.rootdisk_digest,
                record.created_at,
                record.updated_at,
            ],
        )?;
        Ok(())
    }

    /// Update instance phase.
    pub fn set_instance_phase(
        &self,
        instance_id: &str,
        phase: InstancePhase,
    ) -> Result<(), StateStoreError> {
        let now = chrono::Utc::now().timestamp();
        self.conn.execute(
            "UPDATE instances SET phase = ?1, updated_at = ?2 WHERE instance_id = ?3",
            params![phase.as_str(), now, instance_id],
        )?;
        Ok(())
    }

    /// Delete an instance record.
    pub fn delete_instance(&self, instance_id: &str) -> Result<(), StateStoreError> {
        self.conn.execute(
            "DELETE FROM instances WHERE instance_id = ?1",
            params![instance_id],
        )?;
        Ok(())
    }

    /// List all instances.
    pub fn list_instances(&self) -> Result<Vec<InstanceRecord>, StateStoreError> {
        let mut stmt = self.conn.prepare(
            "SELECT instance_id, phase, spec_revision, boot_id, socket_path, rootdisk_digest, created_at, updated_at
             FROM instances ORDER BY created_at",
        )?;

        let records = stmt
            .query_map([], |row| {
                let phase_str: String = row.get(1)?;
                let phase = InstancePhase::from_str(&phase_str).unwrap_or(InstancePhase::Failed);

                Ok(InstanceRecord {
                    instance_id: row.get(0)?,
                    phase,
                    spec_revision: row.get(2)?,
                    boot_id: row.get(3)?,
                    socket_path: row.get(4)?,
                    rootdisk_digest: row.get(5)?,
                    created_at: row.get(6)?,
                    updated_at: row.get(7)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;

        Ok(records)
    }

    /// List instances by phase.
    pub fn list_instances_by_phase(
        &self,
        phase: InstancePhase,
    ) -> Result<Vec<InstanceRecord>, StateStoreError> {
        let mut stmt = self.conn.prepare(
            "SELECT instance_id, phase, spec_revision, boot_id, socket_path, rootdisk_digest, created_at, updated_at
             FROM instances WHERE phase = ?1 ORDER BY created_at",
        )?;

        let records = stmt
            .query_map(params![phase.as_str()], |row| {
                let phase_str: String = row.get(1)?;
                let phase = InstancePhase::from_str(&phase_str).unwrap_or(InstancePhase::Failed);

                Ok(InstanceRecord {
                    instance_id: row.get(0)?,
                    phase,
                    spec_revision: row.get(2)?,
                    boot_id: row.get(3)?,
                    socket_path: row.get(4)?,
                    rootdisk_digest: row.get(5)?,
                    created_at: row.get(6)?,
                    updated_at: row.get(7)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;

        Ok(records)
    }

    /// Count instances by phase.
    pub fn count_instances_by_phase(&self, phase: InstancePhase) -> Result<i64, StateStoreError> {
        let count: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM instances WHERE phase = ?1",
            params![phase.as_str()],
            |row| row.get(0),
        )?;
        Ok(count)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_state_store_node_state() {
        let store = StateStore::open_in_memory().unwrap();

        // Default state
        let state = store.get_node_state().unwrap();
        assert_eq!(state.plan_version, 0);
        assert!(state.event_cursor.is_none());

        // Update state
        store.set_plan_version(42).unwrap();
        store.set_event_cursor("cursor-123").unwrap();

        let state = store.get_node_state().unwrap();
        assert_eq!(state.plan_version, 42);
        assert_eq!(state.event_cursor, Some("cursor-123".to_string()));
    }

    #[test]
    fn test_state_store_instances() {
        let store = StateStore::open_in_memory().unwrap();

        let record = InstanceRecord {
            instance_id: "inst-123".to_string(),
            phase: InstancePhase::Running,
            spec_revision: 1,
            boot_id: "boot-abc".to_string(),
            socket_path: Some("/run/fc.sock".to_string()),
            rootdisk_digest: Some("sha256:abc".to_string()),
            created_at: 1000,
            updated_at: 1000,
        };

        // Insert
        store.upsert_instance(&record).unwrap();

        // Get
        let fetched = store.get_instance("inst-123").unwrap().unwrap();
        assert_eq!(fetched.instance_id, "inst-123");
        assert_eq!(fetched.phase, InstancePhase::Running);

        // Update phase
        store
            .set_instance_phase("inst-123", InstancePhase::Stopped)
            .unwrap();
        let fetched = store.get_instance("inst-123").unwrap().unwrap();
        assert_eq!(fetched.phase, InstancePhase::Stopped);

        // List
        let all = store.list_instances().unwrap();
        assert_eq!(all.len(), 1);

        // Delete
        store.delete_instance("inst-123").unwrap();
        assert!(store.get_instance("inst-123").unwrap().is_none());
    }

    #[test]
    fn test_instance_phase_roundtrip() {
        for phase in [
            InstancePhase::Creating,
            InstancePhase::Starting,
            InstancePhase::Running,
            InstancePhase::Stopping,
            InstancePhase::Stopped,
            InstancePhase::Failed,
        ] {
            let s = phase.as_str();
            let parsed = InstancePhase::from_str(s).unwrap();
            assert_eq!(parsed, phase);
        }
    }
}
