use std::{
    fs,
    path::{Path, PathBuf},
    sync::{Mutex, MutexGuard},
    time::Duration,
};

use apitest_core::{ApiDefinition, EntityId, Environment, Project, ProtocolKind};
use rusqlite::{Connection, OptionalExtension, params};
use uuid::Uuid;

use crate::StorageError;

const SCHEMA_VERSION: i64 = 1;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DefinitionSummary {
    pub id: EntityId,
    pub name: String,
    pub protocol: ProtocolKind,
}

pub struct Database {
    connection: Mutex<Connection>,
    path: Option<PathBuf>,
}

impl Database {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, StorageError> {
        let path = path.as_ref().to_path_buf();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|error| StorageError::io(parent, error))?;
        }
        let connection = Connection::open(&path)?;
        let database = Self {
            connection: Mutex::new(connection),
            path: Some(path),
        };
        database.initialize()?;
        Ok(database)
    }

    pub fn open_in_memory() -> Result<Self, StorageError> {
        let database = Self {
            connection: Mutex::new(Connection::open_in_memory()?),
            path: None,
        };
        database.initialize()?;
        Ok(database)
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    fn connection(&self) -> Result<MutexGuard<'_, Connection>, StorageError> {
        self.connection.lock().map_err(|_| StorageError::Poisoned)
    }

    fn initialize(&self) -> Result<(), StorageError> {
        let connection = self.connection()?;
        connection.execute_batch(
            "PRAGMA foreign_keys = ON;
             PRAGMA journal_mode = WAL;
             PRAGMA synchronous = NORMAL;
             CREATE TABLE IF NOT EXISTS metadata (
                 key TEXT PRIMARY KEY,
                 value TEXT NOT NULL
             );
             CREATE TABLE IF NOT EXISTS projects (
                 id TEXT PRIMARY KEY,
                 name TEXT NOT NULL,
                 updated_at TEXT NOT NULL,
                 document TEXT NOT NULL
             );
             CREATE TABLE IF NOT EXISTS definitions (
                 id TEXT PRIMARY KEY,
                 project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
                 name TEXT NOT NULL,
                 protocol TEXT NOT NULL,
                 updated_at TEXT NOT NULL,
                 document TEXT NOT NULL
             );
             CREATE INDEX IF NOT EXISTS definitions_project_idx
                 ON definitions(project_id, updated_at);
             CREATE VIRTUAL TABLE IF NOT EXISTS definitions_fts USING fts5(
                 id UNINDEXED,
                 project_id UNINDEXED,
                 name,
                 description
             );
             CREATE TABLE IF NOT EXISTS environments (
                 id TEXT PRIMARY KEY,
                 project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
                 name TEXT NOT NULL,
                 document TEXT NOT NULL
             );
             CREATE INDEX IF NOT EXISTS environments_project_idx
                 ON environments(project_id, name);
             CREATE TABLE IF NOT EXISTS request_cases (
                 id TEXT PRIMARY KEY,
                 project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
                 definition_id TEXT NOT NULL,
                 name TEXT NOT NULL,
                 document TEXT NOT NULL
             );
             CREATE TABLE IF NOT EXISTS run_records (
                 id TEXT PRIMARY KEY,
                 project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
                 started_at TEXT NOT NULL,
                 state TEXT NOT NULL,
                 document TEXT NOT NULL
             );
             CREATE INDEX IF NOT EXISTS run_records_project_idx
                 ON run_records(project_id, started_at DESC);
             CREATE TABLE IF NOT EXISTS settings (
                 key TEXT PRIMARY KEY,
                 value TEXT NOT NULL
             );",
        )?;
        connection.execute(
            "INSERT INTO metadata(key, value) VALUES('schema_version', ?1)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            [SCHEMA_VERSION.to_string()],
        )?;
        Ok(())
    }

    pub fn schema_version(&self) -> Result<i64, StorageError> {
        let connection = self.connection()?;
        let version = connection.query_row(
            "SELECT value FROM metadata WHERE key = 'schema_version'",
            [],
            |row| row.get::<_, String>(0),
        )?;
        version
            .parse()
            .map_err(|error| StorageError::Secret(format!("invalid schema version: {error}")))
    }

    pub fn save_project(&self, project: &Project) -> Result<(), StorageError> {
        let document = serde_json::to_string(project)?;
        let connection = self.connection()?;
        connection.execute(
            "INSERT INTO projects(id, name, updated_at, document)
             VALUES(?1, ?2, ?3, ?4)
             ON CONFLICT(id) DO UPDATE SET
                 name = excluded.name,
                 updated_at = excluded.updated_at,
                 document = excluded.document",
            params![
                project.id.to_string(),
                project.name,
                project.updated_at.to_rfc3339(),
                document
            ],
        )?;
        Ok(())
    }

    pub fn list_projects(&self) -> Result<Vec<Project>, StorageError> {
        let connection = self.connection()?;
        let mut statement = connection.prepare("SELECT document FROM projects ORDER BY name")?;
        let documents = statement.query_map([], |row| row.get::<_, String>(0))?;
        deserialize_rows(documents)
    }

    pub fn get_project(&self, id: EntityId) -> Result<Option<Project>, StorageError> {
        let connection = self.connection()?;
        let document = connection
            .query_row(
                "SELECT document FROM projects WHERE id = ?1",
                [id.to_string()],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        document
            .map(|value| serde_json::from_str(&value).map_err(StorageError::from))
            .transpose()
    }

    pub fn save_definition(
        &self,
        project_id: EntityId,
        definition: &ApiDefinition,
    ) -> Result<(), StorageError> {
        let document = serde_json::to_string(definition)?;
        let mut connection = self.connection()?;
        let transaction = connection.transaction()?;
        transaction.execute(
            "INSERT INTO definitions(id, project_id, name, protocol, updated_at, document)
             VALUES(?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(id) DO UPDATE SET
                 project_id = excluded.project_id,
                 name = excluded.name,
                 protocol = excluded.protocol,
                 updated_at = excluded.updated_at,
                 document = excluded.document",
            params![
                definition.id.to_string(),
                project_id.to_string(),
                definition.name,
                protocol_name(definition.protocol.kind()),
                definition.updated_at.to_rfc3339(),
                document
            ],
        )?;
        transaction.execute(
            "DELETE FROM definitions_fts WHERE id = ?1",
            [definition.id.to_string()],
        )?;
        transaction.execute(
            "INSERT INTO definitions_fts(id, project_id, name, description)
             VALUES(?1, ?2, ?3, ?4)",
            params![
                definition.id.to_string(),
                project_id.to_string(),
                definition.name,
                definition.description_markdown
            ],
        )?;
        transaction.commit()?;
        Ok(())
    }

    pub fn list_definitions(
        &self,
        project_id: EntityId,
    ) -> Result<Vec<ApiDefinition>, StorageError> {
        let connection = self.connection()?;
        let mut statement = connection
            .prepare("SELECT document FROM definitions WHERE project_id = ?1 ORDER BY rowid")?;
        let documents =
            statement.query_map([project_id.to_string()], |row| row.get::<_, String>(0))?;
        deserialize_rows(documents)
    }

    pub fn get_definition(&self, id: EntityId) -> Result<Option<ApiDefinition>, StorageError> {
        let connection = self.connection()?;
        let document = connection
            .query_row(
                "SELECT document FROM definitions WHERE id = ?1",
                [id.to_string()],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        document
            .map(|value| serde_json::from_str(&value).map_err(StorageError::from))
            .transpose()
    }

    pub fn search_definitions(
        &self,
        project_id: EntityId,
        query: &str,
        limit: usize,
    ) -> Result<Vec<DefinitionSummary>, StorageError> {
        if query.trim().is_empty() || limit == 0 {
            return Ok(Vec::new());
        }
        let search = fts_prefix_query(query);
        let connection = self.connection()?;
        let mut statement = connection.prepare(
            "SELECT d.id, d.name, d.protocol
             FROM definitions_fts
             JOIN definitions d ON d.id = definitions_fts.id
             WHERE definitions_fts MATCH ?1
               AND definitions_fts.project_id = ?2
             ORDER BY rank
             LIMIT ?3",
        )?;
        let rows = statement.query_map(
            params![search, project_id.to_string(), limit as i64],
            |row| {
                let id: String = row.get(0)?;
                let name: String = row.get(1)?;
                let protocol: String = row.get(2)?;
                Ok((id, name, protocol))
            },
        )?;

        let mut summaries = Vec::new();
        for row in rows {
            let (id, name, protocol) = row?;
            let uuid = Uuid::parse_str(&id).map_err(|error| {
                StorageError::Secret(format!("invalid definition id in database: {error}"))
            })?;
            summaries.push(DefinitionSummary {
                id: EntityId::from_uuid(uuid),
                name,
                protocol: parse_protocol_name(&protocol)?,
            });
        }
        Ok(summaries)
    }

    pub fn save_environment(
        &self,
        project_id: EntityId,
        environment: &Environment,
    ) -> Result<(), StorageError> {
        let document = serde_json::to_string(environment)?;
        let connection = self.connection()?;
        connection.execute(
            "INSERT INTO environments(id, project_id, name, document)
             VALUES(?1, ?2, ?3, ?4)
             ON CONFLICT(id) DO UPDATE SET
                 project_id = excluded.project_id,
                 name = excluded.name,
                 document = excluded.document",
            params![
                environment.id.to_string(),
                project_id.to_string(),
                environment.name,
                document
            ],
        )?;
        Ok(())
    }

    pub fn list_environments(
        &self,
        project_id: EntityId,
    ) -> Result<Vec<Environment>, StorageError> {
        let connection = self.connection()?;
        let mut statement = connection
            .prepare("SELECT document FROM environments WHERE project_id = ?1 ORDER BY name")?;
        let documents =
            statement.query_map([project_id.to_string()], |row| row.get::<_, String>(0))?;
        deserialize_rows(documents)
    }

    pub fn set_setting<T: serde::Serialize>(
        &self,
        key: &str,
        value: &T,
    ) -> Result<(), StorageError> {
        let value = serde_json::to_string(value)?;
        let connection = self.connection()?;
        connection.execute(
            "INSERT INTO settings(key, value) VALUES(?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;
        Ok(())
    }

    pub fn get_setting<T: serde::de::DeserializeOwned>(
        &self,
        key: &str,
    ) -> Result<Option<T>, StorageError> {
        let connection = self.connection()?;
        let value = connection
            .query_row("SELECT value FROM settings WHERE key = ?1", [key], |row| {
                row.get::<_, String>(0)
            })
            .optional()?;
        value
            .map(|document| serde_json::from_str(&document).map_err(StorageError::from))
            .transpose()
    }

    pub fn create_backup(&self, destination: impl AsRef<Path>) -> Result<(), StorageError> {
        let destination = destination.as_ref();
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent).map_err(|error| StorageError::io(parent, error))?;
        }
        let source = self.connection()?;
        let mut target = Connection::open(destination)?;
        let backup = rusqlite::backup::Backup::new(&source, &mut target)?;
        backup.run_to_completion(128, Duration::from_millis(5), None)?;
        Ok(())
    }
}

fn deserialize_rows<T>(
    rows: rusqlite::MappedRows<'_, impl FnMut(&rusqlite::Row<'_>) -> rusqlite::Result<String>>,
) -> Result<Vec<T>, StorageError>
where
    T: serde::de::DeserializeOwned,
{
    let mut values = Vec::new();
    for row in rows {
        let document = row?;
        values.push(serde_json::from_str(&document)?);
    }
    Ok(values)
}

fn protocol_name(kind: ProtocolKind) -> &'static str {
    match kind {
        ProtocolKind::Http => "http",
        ProtocolKind::GraphQl => "graphql",
        ProtocolKind::Sse => "sse",
        ProtocolKind::WebSocket => "websocket",
        ProtocolKind::Grpc => "grpc",
    }
}

fn parse_protocol_name(value: &str) -> Result<ProtocolKind, StorageError> {
    match value {
        "http" => Ok(ProtocolKind::Http),
        "graphql" => Ok(ProtocolKind::GraphQl),
        "sse" => Ok(ProtocolKind::Sse),
        "websocket" => Ok(ProtocolKind::WebSocket),
        "grpc" => Ok(ProtocolKind::Grpc),
        other => Err(StorageError::Secret(format!(
            "unknown protocol in database: {other}"
        ))),
    }
}

fn fts_prefix_query(query: &str) -> String {
    query
        .split_whitespace()
        .filter(|token| !token.is_empty())
        .map(|token| format!("\"{}\"*", token.replace('"', "\"\"")))
        .collect::<Vec<_>>()
        .join(" AND ")
}
