use std::{
    fs,
    path::{Path, PathBuf},
    sync::{Mutex, MutexGuard},
    time::Duration,
};

use apitest_core::{
    ApiDefinition, EntityId, Environment, MockProfile, Project, ProjectNode, ProjectNodeKind,
    ProtocolKind, RequestCase, RunRecord, TestScenario,
};
use chrono::{DateTime, TimeDelta, Utc};
use rusqlite::{Connection, OptionalExtension, params};
use uuid::Uuid;

use crate::StorageError;

const SCHEMA_VERSION: i64 = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PageRequest {
    pub offset: usize,
    pub limit: usize,
}

impl PageRequest {
    pub fn new(offset: usize, limit: usize) -> Self {
        Self {
            offset,
            limit: limit.clamp(1, 500),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Page<T> {
    pub items: Vec<T>,
    pub total: usize,
}

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
    pub fn backup_file(
        source: impl AsRef<Path>,
        destination: impl AsRef<Path>,
    ) -> Result<(), StorageError> {
        let source_path = source.as_ref();
        let destination = destination.as_ref();
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent).map_err(|error| StorageError::io(parent, error))?;
        }
        let source = Connection::open(source_path)?;
        let mut target = Connection::open(destination)?;
        let backup = rusqlite::backup::Backup::new(&source, &mut target)?;
        backup.run_to_completion(128, Duration::from_millis(5), None)?;
        Ok(())
    }

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
        let mut connection = self.connection()?;
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
                 definition_id TEXT NOT NULL REFERENCES definitions(id) ON DELETE CASCADE,
                 name TEXT NOT NULL,
                 document TEXT NOT NULL
             );
             CREATE INDEX IF NOT EXISTS request_cases_definition_idx
                 ON request_cases(project_id, definition_id, name);
             CREATE TABLE IF NOT EXISTS project_nodes (
                 id TEXT PRIMARY KEY,
                 project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
                 parent_id TEXT,
                 entity_id TEXT,
                 kind TEXT NOT NULL,
                 name TEXT NOT NULL,
                 sort_order INTEGER NOT NULL,
                 document TEXT NOT NULL
             );
             CREATE INDEX IF NOT EXISTS project_nodes_parent_idx
                 ON project_nodes(project_id, parent_id, sort_order, name);
             CREATE INDEX IF NOT EXISTS project_nodes_entity_idx
                 ON project_nodes(project_id, entity_id, kind);
             CREATE TABLE IF NOT EXISTS scenarios (
                 id TEXT PRIMARY KEY,
                 project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
                 name TEXT NOT NULL,
                 document TEXT NOT NULL
             );
             CREATE INDEX IF NOT EXISTS scenarios_project_idx
                 ON scenarios(project_id, name);
             CREATE TABLE IF NOT EXISTS mock_profiles (
                 id TEXT PRIMARY KEY,
                 project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
                 name TEXT NOT NULL,
                 document TEXT NOT NULL
             );
             CREATE INDEX IF NOT EXISTS mock_profiles_project_idx
                 ON mock_profiles(project_id, name);
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
        let stored: Option<String> = connection
            .query_row(
                "SELECT value FROM metadata WHERE key = 'schema_version'",
                [],
                |row| row.get(0),
            )
            .optional()?;
        if stored.as_deref() != Some(&SCHEMA_VERSION.to_string()) {
            // v3 keys the FTS rows by `definitions.rowid`, so deletes are a
            // rowid lookup instead of a full index scan (`id` is UNINDEXED).
            // Repopulating is idempotent and cheap relative to a migration.
            let transaction = connection.transaction()?;
            transaction.execute("DELETE FROM definitions_fts", [])?;
            transaction.execute(
                "INSERT INTO definitions_fts(rowid, id, project_id, name, description)
                 SELECT rowid, id, project_id, name,
                        COALESCE(json_extract(document, '$.description_markdown'), '')
                 FROM definitions",
                [],
            )?;
            transaction.commit()?;
        }
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
                protocol_name(definition.contract.kind()),
                definition.updated_at.to_rfc3339(),
                document
            ],
        )?;
        refresh_definition_fts(&transaction, project_id, definition)?;
        transaction.commit()?;
        Ok(())
    }

    pub fn save_definition_and_case(
        &self,
        project_id: EntityId,
        definition: &ApiDefinition,
        request_case: &RequestCase,
    ) -> Result<(), StorageError> {
        let definition_document = serde_json::to_string(definition)?;
        let case_document = serde_json::to_string(request_case)?;
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
                protocol_name(definition.contract.kind()),
                definition.updated_at.to_rfc3339(),
                definition_document
            ],
        )?;
        refresh_definition_fts(&transaction, project_id, definition)?;
        transaction.execute(
            "INSERT INTO request_cases(id, project_id, definition_id, name, document)
             VALUES(?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(id) DO UPDATE SET
                 project_id = excluded.project_id,
                 definition_id = excluded.definition_id,
                 name = excluded.name,
                 document = excluded.document",
            params![
                request_case.id.to_string(),
                project_id.to_string(),
                request_case.definition_id.to_string(),
                request_case.name,
                case_document
            ],
        )?;
        upsert_definition_node(&transaction, project_id, definition)?;
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

    pub fn delete_definition(
        &self,
        project_id: EntityId,
        id: EntityId,
    ) -> Result<bool, StorageError> {
        let mut connection = self.connection()?;
        let transaction = connection.transaction()?;
        transaction.execute(
            "DELETE FROM project_nodes
             WHERE project_id = ?1 AND kind = 'request_case'
               AND entity_id IN (
                   SELECT id FROM request_cases
                   WHERE project_id = ?1 AND definition_id = ?2
               )",
            params![project_id.to_string(), id.to_string()],
        )?;
        transaction.execute(
            "DELETE FROM request_cases WHERE project_id = ?1 AND definition_id = ?2",
            params![project_id.to_string(), id.to_string()],
        )?;
        transaction.execute(
            "DELETE FROM definitions_fts WHERE rowid =
                 (SELECT rowid FROM definitions WHERE project_id = ?1 AND id = ?2)",
            params![project_id.to_string(), id.to_string()],
        )?;
        transaction.execute(
            "DELETE FROM project_nodes
             WHERE project_id = ?1 AND entity_id = ?2 AND kind = 'api_definition'",
            params![project_id.to_string(), id.to_string()],
        )?;
        let deleted = transaction.execute(
            "DELETE FROM definitions WHERE project_id = ?1 AND id = ?2",
            params![project_id.to_string(), id.to_string()],
        )?;
        transaction.commit()?;
        Ok(deleted > 0)
    }

    /// `delete_definition` over many ids in one transaction: deleting a
    /// folder of N requests used to run N transactions with a storage flush
    /// between each.
    pub fn delete_definitions(
        &self,
        project_id: EntityId,
        ids: &[EntityId],
    ) -> Result<usize, StorageError> {
        if ids.is_empty() {
            return Ok(0);
        }
        let mut connection = self.connection()?;
        let transaction = connection.transaction()?;
        let project_id = project_id.to_string();
        let mut deleted = 0;
        for id in ids {
            let id = id.to_string();
            transaction
                .prepare_cached(
                    "DELETE FROM project_nodes
                     WHERE project_id = ?1 AND kind = 'request_case'
                       AND entity_id IN (
                           SELECT id FROM request_cases
                           WHERE project_id = ?1 AND definition_id = ?2
                       )",
                )?
                .execute(params![project_id, id])?;
            transaction
                .prepare_cached(
                    "DELETE FROM request_cases WHERE project_id = ?1 AND definition_id = ?2",
                )?
                .execute(params![project_id, id])?;
            transaction
                .prepare_cached(
                    "DELETE FROM definitions_fts WHERE rowid =
                         (SELECT rowid FROM definitions WHERE project_id = ?1 AND id = ?2)",
                )?
                .execute(params![project_id, id])?;
            transaction
                .prepare_cached(
                    "DELETE FROM project_nodes
                     WHERE project_id = ?1 AND entity_id = ?2 AND kind = 'api_definition'",
                )?
                .execute(params![project_id, id])?;
            deleted += transaction
                .prepare_cached("DELETE FROM definitions WHERE project_id = ?1 AND id = ?2")?
                .execute(params![project_id, id])?;
        }
        transaction.commit()?;
        Ok(deleted)
    }

    /// Every definition beneath `folder`, at any depth.
    ///
    /// Callers need this before deleting a folder so they can tell the user how
    /// much goes with it and clean up the matching secrets.
    pub fn definitions_under(
        &self,
        project_id: EntityId,
        folder: EntityId,
    ) -> Result<Vec<EntityId>, StorageError> {
        let connection = self.connection()?;
        let mut statement = connection.prepare(
            "WITH RECURSIVE descendants(id) AS (
                 SELECT id FROM project_nodes
                 WHERE project_id = ?1 AND id = ?2
                 UNION ALL
                 SELECT n.id FROM project_nodes n
                 JOIN descendants d ON n.parent_id = d.id
                 WHERE n.project_id = ?1
             )
             SELECT n.entity_id FROM project_nodes n
             JOIN descendants d ON n.id = d.id
             WHERE n.kind = 'api_definition' AND n.entity_id IS NOT NULL",
        )?;
        let rows = statement
            .query_map(params![project_id.to_string(), folder.to_string()], |row| {
                row.get::<_, String>(0)
            })?;
        let mut ids = Vec::new();
        for row in rows {
            let raw = row?;
            let uuid = Uuid::parse_str(&raw).map_err(|error| {
                StorageError::Secret(format!("invalid definition id in database: {error}"))
            })?;
            ids.push(EntityId::from_uuid(uuid));
        }
        Ok(ids)
    }

    /// Remove a node and everything nested under it.
    ///
    /// Definitions are deleted through `delete_definition` first so their cases
    /// and search rows go with them; this only clears the tree itself.
    pub fn delete_project_node(
        &self,
        project_id: EntityId,
        id: EntityId,
    ) -> Result<usize, StorageError> {
        let mut connection = self.connection()?;
        let transaction = connection.transaction()?;
        let deleted = transaction.execute(
            "WITH RECURSIVE descendants(id) AS (
                 SELECT id FROM project_nodes
                 WHERE project_id = ?1 AND id = ?2
                 UNION ALL
                 SELECT n.id FROM project_nodes n
                 JOIN descendants d ON n.parent_id = d.id
                 WHERE n.project_id = ?1
             )
             DELETE FROM project_nodes
             WHERE project_id = ?1 AND id IN (SELECT id FROM descendants)",
            params![project_id.to_string(), id.to_string()],
        )?;
        transaction.commit()?;
        Ok(deleted)
    }

    pub fn delete_scenario(
        &self,
        project_id: EntityId,
        id: EntityId,
    ) -> Result<bool, StorageError> {
        let connection = self.connection()?;
        let deleted = connection.execute(
            "DELETE FROM scenarios WHERE project_id = ?1 AND id = ?2",
            params![project_id.to_string(), id.to_string()],
        )?;
        Ok(deleted > 0)
    }

    pub fn delete_mock_profile(
        &self,
        project_id: EntityId,
        id: EntityId,
    ) -> Result<bool, StorageError> {
        let connection = self.connection()?;
        let deleted = connection.execute(
            "DELETE FROM mock_profiles WHERE project_id = ?1 AND id = ?2",
            params![project_id.to_string(), id.to_string()],
        )?;
        Ok(deleted > 0)
    }

    pub fn save_request_case(
        &self,
        project_id: EntityId,
        request_case: &RequestCase,
    ) -> Result<(), StorageError> {
        let document = serde_json::to_string(request_case)?;
        let connection = self.connection()?;
        connection.execute(
            "INSERT INTO request_cases(id, project_id, definition_id, name, document)
             VALUES(?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(id) DO UPDATE SET
                 project_id = excluded.project_id,
                 definition_id = excluded.definition_id,
                 name = excluded.name,
                 document = excluded.document",
            params![
                request_case.id.to_string(),
                project_id.to_string(),
                request_case.definition_id.to_string(),
                request_case.name,
                document
            ],
        )?;
        Ok(())
    }

    pub fn list_request_cases(
        &self,
        project_id: EntityId,
        definition_id: EntityId,
        page: PageRequest,
    ) -> Result<Page<RequestCase>, StorageError> {
        let connection = self.connection()?;
        let total = connection.query_row(
            "SELECT COUNT(*) FROM request_cases WHERE project_id = ?1 AND definition_id = ?2",
            params![project_id.to_string(), definition_id.to_string()],
            |row| row.get::<_, i64>(0),
        )? as usize;
        let mut statement = connection.prepare_cached(
            "SELECT document FROM request_cases
             WHERE project_id = ?1 AND definition_id = ?2
             ORDER BY name, id LIMIT ?3 OFFSET ?4",
        )?;
        let rows = statement.query_map(
            params![
                project_id.to_string(),
                definition_id.to_string(),
                page.limit as i64,
                page.offset as i64
            ],
            |row| row.get::<_, String>(0),
        )?;
        Ok(Page {
            items: deserialize_rows(rows)?,
            total,
        })
    }

    /// The first request case of every definition in the project, in one
    /// query. Loading a workspace used to issue one query per definition.
    pub fn first_request_cases(
        &self,
        project_id: EntityId,
    ) -> Result<Vec<RequestCase>, StorageError> {
        let connection = self.connection()?;
        let mut statement = connection.prepare_cached(
            "SELECT document FROM (
                 SELECT document, ROW_NUMBER() OVER (
                     PARTITION BY definition_id ORDER BY name, id
                 ) AS row_index
                 FROM request_cases WHERE project_id = ?1
             ) WHERE row_index = 1",
        )?;
        let rows = statement.query_map([project_id.to_string()], |row| row.get::<_, String>(0))?;
        deserialize_rows(rows)
    }

    pub fn save_project_node(&self, node: &ProjectNode) -> Result<(), StorageError> {
        let document = serde_json::to_string(node)?;
        let connection = self.connection()?;
        connection.execute(
            "INSERT INTO project_nodes(
                 id, project_id, parent_id, entity_id, kind, name, sort_order, document
             ) VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             ON CONFLICT(id) DO UPDATE SET
                 project_id = excluded.project_id,
                 parent_id = excluded.parent_id,
                 entity_id = excluded.entity_id,
                 kind = excluded.kind,
                 name = excluded.name,
                 sort_order = excluded.sort_order,
                 document = excluded.document",
            params![
                node.id.to_string(),
                node.project_id.to_string(),
                node.parent_id.map(|id| id.to_string()),
                node.entity_id.map(|id| id.to_string()),
                project_node_kind_name(node.kind),
                node.name,
                node.sort_order,
                document
            ],
        )?;
        Ok(())
    }

    pub fn ensure_definition_node(
        &self,
        project_id: EntityId,
        definition: &ApiDefinition,
    ) -> Result<(), StorageError> {
        let connection = self.connection()?;
        upsert_definition_node(&connection, project_id, definition)
    }

    /// `ensure_definition_node` for a whole workspace load, in one transaction
    /// instead of one implicit commit per definition.
    pub fn ensure_definition_nodes(
        &self,
        project_id: EntityId,
        definitions: &[ApiDefinition],
    ) -> Result<(), StorageError> {
        let mut connection = self.connection()?;
        let transaction = connection.transaction()?;
        for definition in definitions {
            upsert_definition_node(&transaction, project_id, definition)?;
        }
        transaction.commit()?;
        Ok(())
    }

    pub fn list_project_nodes(
        &self,
        project_id: EntityId,
        parent_id: Option<EntityId>,
        page: PageRequest,
    ) -> Result<Page<ProjectNode>, StorageError> {
        let project_id = project_id.to_string();
        let parent_id = parent_id.map(|id| id.to_string());
        let connection = self.connection()?;
        let total = connection.query_row(
            "SELECT COUNT(*) FROM project_nodes
             WHERE project_id = ?1
               AND ((?2 IS NULL AND parent_id IS NULL) OR parent_id = ?2)",
            params![project_id, parent_id],
            |row| row.get::<_, i64>(0),
        )? as usize;
        let mut statement = connection.prepare_cached(
            "SELECT document FROM project_nodes
             WHERE project_id = ?1
               AND ((?2 IS NULL AND parent_id IS NULL) OR parent_id = ?2)
             ORDER BY sort_order, name, id LIMIT ?3 OFFSET ?4",
        )?;
        let rows = statement.query_map(
            params![project_id, parent_id, page.limit as i64, page.offset as i64],
            |row| row.get::<_, String>(0),
        )?;
        Ok(Page {
            items: deserialize_rows(rows)?,
            total,
        })
    }

    pub fn save_scenario(
        &self,
        project_id: EntityId,
        scenario: &TestScenario,
    ) -> Result<(), StorageError> {
        let connection = self.connection()?;
        save_project_document(
            &connection,
            "scenarios",
            scenario.id,
            project_id,
            &scenario.name,
            scenario,
        )
    }

    pub fn list_scenarios(&self, project_id: EntityId) -> Result<Vec<TestScenario>, StorageError> {
        let connection = self.connection()?;
        list_project_documents(&connection, "scenarios", project_id)
    }

    pub fn save_mock_profile(
        &self,
        project_id: EntityId,
        profile: &MockProfile,
    ) -> Result<(), StorageError> {
        let connection = self.connection()?;
        save_project_document(
            &connection,
            "mock_profiles",
            profile.id,
            project_id,
            &profile.name,
            profile,
        )
    }

    pub fn list_mock_profiles(
        &self,
        project_id: EntityId,
    ) -> Result<Vec<MockProfile>, StorageError> {
        let connection = self.connection()?;
        list_project_documents(&connection, "mock_profiles", project_id)
    }

    pub fn save_run_record(
        &self,
        project_id: EntityId,
        record: &RunRecord,
    ) -> Result<(), StorageError> {
        let document = serde_json::to_string(record)?;
        let connection = self.connection()?;
        let mut statement = connection.prepare_cached(
            "INSERT INTO run_records(id, project_id, started_at, state, document)
             VALUES(?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(id) DO UPDATE SET
                 project_id = excluded.project_id,
                 started_at = excluded.started_at,
                 state = excluded.state,
                 document = excluded.document",
        )?;
        statement.execute(params![
            record.id.to_string(),
            project_id.to_string(),
            record.started_at.to_rfc3339(),
            run_state_name(record.state),
            document
        ])?;
        Ok(())
    }

    pub fn list_run_records(
        &self,
        project_id: EntityId,
        page: PageRequest,
    ) -> Result<Page<RunRecord>, StorageError> {
        let project_id = project_id.to_string();
        let connection = self.connection()?;
        let total = connection.query_row(
            "SELECT COUNT(*) FROM run_records WHERE project_id = ?1",
            [&project_id],
            |row| row.get::<_, i64>(0),
        )? as usize;
        let mut statement = connection.prepare_cached(
            "SELECT document FROM run_records WHERE project_id = ?1
             ORDER BY started_at DESC, id DESC LIMIT ?2 OFFSET ?3",
        )?;
        let rows = statement.query_map(
            params![project_id, page.limit as i64, page.offset as i64],
            |row| row.get::<_, String>(0),
        )?;
        Ok(Page {
            items: deserialize_rows(rows)?,
            total,
        })
    }

    pub fn prune_run_records(
        &self,
        project_id: EntityId,
        max_records: usize,
        max_age_days: i64,
        now: DateTime<Utc>,
    ) -> Result<usize, StorageError> {
        self.prune_run_records_with_body_paths(project_id, max_records, max_age_days, now)
            .map(|(deleted, _)| deleted)
    }

    pub fn prune_run_records_with_body_paths(
        &self,
        project_id: EntityId,
        max_records: usize,
        max_age_days: i64,
        now: DateTime<Utc>,
    ) -> Result<(usize, Vec<PathBuf>), StorageError> {
        let mut connection = self.connection()?;
        let transaction = connection.transaction()?;
        let cutoff = now
            .checked_sub_signed(TimeDelta::days(max_age_days.max(0)))
            .unwrap_or(now);
        let project_id = project_id.to_string();
        let body_paths = {
            // Only the body path is needed; deserializing every pruned record
            // just to read one field was pure waste.
            let mut statement = transaction.prepare_cached(
                "SELECT json_extract(document, '$.body_path') FROM run_records
                 WHERE project_id = ?1 AND (
                     started_at < ?2 OR id NOT IN (
                         SELECT id FROM run_records WHERE project_id = ?1
                         ORDER BY started_at DESC, id DESC LIMIT ?3
                     )
                 )",
            )?;
            let rows = statement.query_map(
                params![project_id, cutoff.to_rfc3339(), max_records as i64],
                |row| row.get::<_, Option<String>>(0),
            )?;
            let mut paths = Vec::new();
            for row in rows {
                if let Some(path) = row? {
                    paths.push(PathBuf::from(path));
                }
            }
            paths
        };
        let mut deleted = transaction.execute(
            "DELETE FROM run_records WHERE project_id = ?1 AND started_at < ?2",
            params![project_id, cutoff.to_rfc3339()],
        )?;
        deleted += transaction.execute(
            "DELETE FROM run_records
             WHERE project_id = ?1 AND id NOT IN (
                 SELECT id FROM run_records WHERE project_id = ?1
                 ORDER BY started_at DESC, id DESC LIMIT ?2
             )",
            params![project_id, max_records as i64],
        )?;
        transaction.commit()?;
        Ok((deleted, body_paths))
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
        let mut statement = connection.prepare_cached(
            "SELECT d.id, d.name, d.protocol
             FROM definitions_fts
             JOIN definitions d ON d.rowid = definitions_fts.rowid
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

    pub fn delete_environment(
        &self,
        project_id: EntityId,
        id: EntityId,
    ) -> Result<bool, StorageError> {
        let connection = self.connection()?;
        let deleted = connection.execute(
            "DELETE FROM environments WHERE project_id = ?1 AND id = ?2",
            params![project_id.to_string(), id.to_string()],
        )?;
        Ok(deleted > 0)
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

fn upsert_definition_node(
    connection: &Connection,
    project_id: EntityId,
    definition: &ApiDefinition,
) -> Result<(), StorageError> {
    let existing = connection
        .query_row(
            "SELECT document FROM project_nodes
             WHERE project_id = ?1 AND entity_id = ?2 AND kind = 'api_definition'
             ORDER BY sort_order, id LIMIT 1",
            params![project_id.to_string(), definition.id.to_string()],
            |row| row.get::<_, String>(0),
        )
        .optional()?;
    let mut node = match existing {
        Some(document) => serde_json::from_str::<ProjectNode>(&document)?,
        None => ProjectNode {
            id: EntityId::new(),
            project_id,
            parent_id: None,
            entity_id: Some(definition.id),
            kind: ProjectNodeKind::ApiDefinition,
            name: definition.name.clone(),
            sort_order: definition.created_at.timestamp_millis(),
        },
    };
    node.name = definition.name.clone();
    let document = serde_json::to_string(&node)?;
    connection.execute(
        "INSERT INTO project_nodes(
             id, project_id, parent_id, entity_id, kind, name, sort_order, document
         ) VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
         ON CONFLICT(id) DO UPDATE SET
             project_id = excluded.project_id,
             parent_id = excluded.parent_id,
             entity_id = excluded.entity_id,
             kind = excluded.kind,
             name = excluded.name,
             sort_order = excluded.sort_order,
             document = excluded.document",
        params![
            node.id.to_string(),
            node.project_id.to_string(),
            node.parent_id.map(|id| id.to_string()),
            node.entity_id.map(|id| id.to_string()),
            project_node_kind_name(node.kind),
            node.name,
            node.sort_order,
            document
        ],
    )?;
    Ok(())
}

/// Replace the FTS row for `definition`, keyed by the `definitions` rowid so
/// the delete is a direct lookup. The definitions row must already exist.
fn refresh_definition_fts(
    connection: &Connection,
    project_id: EntityId,
    definition: &ApiDefinition,
) -> Result<(), StorageError> {
    connection
        .prepare_cached(
            "DELETE FROM definitions_fts WHERE rowid =
                 (SELECT rowid FROM definitions WHERE id = ?1)",
        )?
        .execute([definition.id.to_string()])?;
    connection
        .prepare_cached(
            "INSERT INTO definitions_fts(rowid, id, project_id, name, description)
             SELECT rowid, ?1, ?2, ?3, ?4 FROM definitions WHERE id = ?1",
        )?
        .execute(params![
            definition.id.to_string(),
            project_id.to_string(),
            definition.name,
            definition.description_markdown
        ])?;
    Ok(())
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

fn save_project_document<T: serde::Serialize>(
    connection: &Connection,
    table: &str,
    id: EntityId,
    project_id: EntityId,
    name: &str,
    value: &T,
) -> Result<(), StorageError> {
    let document = serde_json::to_string(value)?;
    let sql = format!(
        "INSERT INTO {table}(id, project_id, name, document)
         VALUES(?1, ?2, ?3, ?4)
         ON CONFLICT(id) DO UPDATE SET
             project_id = excluded.project_id,
             name = excluded.name,
             document = excluded.document"
    );
    connection.execute(
        &sql,
        params![id.to_string(), project_id.to_string(), name, document],
    )?;
    Ok(())
}

fn list_project_documents<T: serde::de::DeserializeOwned>(
    connection: &Connection,
    table: &str,
    project_id: EntityId,
) -> Result<Vec<T>, StorageError> {
    let sql = format!("SELECT document FROM {table} WHERE project_id = ?1 ORDER BY name, id");
    let mut statement = connection.prepare(&sql)?;
    let rows = statement.query_map([project_id.to_string()], |row| row.get::<_, String>(0))?;
    deserialize_rows(rows)
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

fn project_node_kind_name(kind: ProjectNodeKind) -> &'static str {
    match kind {
        ProjectNodeKind::Folder => "folder",
        ProjectNodeKind::ApiDefinition => "api_definition",
        ProjectNodeKind::RequestCase => "request_case",
        ProjectNodeKind::TestScenario => "test_scenario",
        ProjectNodeKind::DataModel => "data_model",
    }
}

fn run_state_name(state: apitest_core::RunState) -> &'static str {
    match state {
        apitest_core::RunState::Running => "running",
        apitest_core::RunState::Passed => "passed",
        apitest_core::RunState::Failed => "failed",
        apitest_core::RunState::Cancelled => "cancelled",
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
