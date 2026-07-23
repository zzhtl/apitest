use std::{
    path::{Path, PathBuf},
    sync::{Arc, mpsc},
    thread,
};

#[cfg(test)]
use std::time::Duration;

use apitest_core::{ApiDefinition, EntityId, Environment, RequestCase};
use apitest_storage::{Database, StorageError};
use chrono::Utc;
use thiserror::Error;

pub(crate) struct DatabaseStartup {
    pub database: Database,
    pub legacy_archive: Option<PathBuf>,
}

pub(crate) fn open_workspace_database(
    directory: impl AsRef<Path>,
) -> Result<DatabaseStartup, StorageError> {
    let directory = directory.as_ref();
    let legacy_path = directory.join("apitest.sqlite3");
    let database_path = directory.join("apitest-v2.sqlite3");
    let legacy_archive = if !database_path.exists() && legacy_path.exists() {
        let archive = directory.join("backups").join(format!(
            "apitest-v1-{}.sqlite3",
            Utc::now().format("%Y%m%d-%H%M%S-%3f")
        ));
        Database::backup_file(&legacy_path, &archive)?;
        Some(archive)
    } else {
        None
    };
    let database = Database::open(database_path)?;
    Ok(DatabaseStartup {
        database,
        legacy_archive,
    })
}

// Commands own their documents so the bounded worker can persist immutable snapshots.
#[allow(clippy::large_enum_variant)]
enum StorageCommand {
    SaveRequest {
        project_id: EntityId,
        definition: ApiDefinition,
        request_case: RequestCase,
        revision: u64,
        notify: bool,
    },
    SaveEnvironment {
        project_id: EntityId,
        environment: Environment,
        revision: u64,
        notify: bool,
    },
    ImportRequests {
        project_id: EntityId,
        requests: Vec<(ApiDefinition, RequestCase, u64)>,
    },
    Flush {
        acknowledged: mpsc::Sender<()>,
    },
}

#[derive(Debug)]
// Events return the saved snapshot to update the matching editor revision atomically.
#[allow(clippy::large_enum_variant)]
pub(crate) enum StorageEvent {
    RequestSaved {
        revision: u64,
        definition: ApiDefinition,
        request_case: RequestCase,
        notify: bool,
    },
    EnvironmentSaved {
        revision: u64,
        environment: Environment,
        notify: bool,
    },
    SaveFailed {
        revision: u64,
        entity_id: EntityId,
        error: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub(crate) enum StorageQueueError {
    #[error("the storage queue is full")]
    Full,
    #[error("the storage worker has stopped")]
    Closed,
}

pub(crate) struct StorageWorker {
    commands: Option<mpsc::SyncSender<StorageCommand>>,
    events: mpsc::Receiver<StorageEvent>,
    task: Option<thread::JoinHandle<()>>,
}

impl StorageWorker {
    pub fn new(database: Arc<Database>) -> Self {
        let (command_sender, command_receiver) = mpsc::sync_channel(64);
        let (event_sender, events) = mpsc::channel();
        let task = thread::Builder::new()
            .name("apitest-storage".into())
            .spawn(move || {
                'commands: while let Ok(command) = command_receiver.recv() {
                    match command {
                        StorageCommand::SaveRequest {
                            project_id,
                            definition,
                            request_case,
                            revision,
                            notify,
                        } => {
                            let event = match database.save_definition_and_case(
                                project_id,
                                &definition,
                                &request_case,
                            ) {
                                Ok(()) => StorageEvent::RequestSaved {
                                    revision,
                                    definition,
                                    request_case,
                                    notify,
                                },
                                Err(error) => StorageEvent::SaveFailed {
                                    revision,
                                    entity_id: definition.id,
                                    error: error.to_string(),
                                },
                            };
                            if event_sender.send(event).is_err() {
                                break;
                            }
                        }
                        StorageCommand::SaveEnvironment {
                            project_id,
                            environment,
                            revision,
                            notify,
                        } => {
                            let event = match database.save_environment(project_id, &environment) {
                                Ok(()) => StorageEvent::EnvironmentSaved {
                                    revision,
                                    environment,
                                    notify,
                                },
                                Err(error) => StorageEvent::SaveFailed {
                                    revision,
                                    entity_id: environment.id,
                                    error: error.to_string(),
                                },
                            };
                            if event_sender.send(event).is_err() {
                                break;
                            }
                        }
                        StorageCommand::ImportRequests {
                            project_id,
                            requests,
                        } => {
                            for (definition, request_case, revision) in requests {
                                let event = match database.save_definition_and_case(
                                    project_id,
                                    &definition,
                                    &request_case,
                                ) {
                                    Ok(()) => StorageEvent::RequestSaved {
                                        revision,
                                        definition,
                                        request_case,
                                        notify: false,
                                    },
                                    Err(error) => StorageEvent::SaveFailed {
                                        revision,
                                        entity_id: definition.id,
                                        error: error.to_string(),
                                    },
                                };
                                if event_sender.send(event).is_err() {
                                    break 'commands;
                                }
                            }
                        }
                        StorageCommand::Flush { acknowledged } => {
                            let _ = acknowledged.send(());
                        }
                    }
                }
            })
            .expect("storage worker thread should initialize");
        Self {
            commands: Some(command_sender),
            events,
            task: Some(task),
        }
    }

    pub fn queue_request(
        &self,
        project_id: EntityId,
        definition: ApiDefinition,
        request_case: RequestCase,
        revision: u64,
        notify: bool,
    ) -> Result<(), StorageQueueError> {
        let command = StorageCommand::SaveRequest {
            project_id,
            definition,
            request_case,
            revision,
            notify,
        };
        self.commands
            .as_ref()
            .ok_or(StorageQueueError::Closed)?
            .try_send(command)
            .map_err(|error| match error {
                mpsc::TrySendError::Full(_) => StorageQueueError::Full,
                mpsc::TrySendError::Disconnected(_) => StorageQueueError::Closed,
            })
    }

    pub fn queue_environment(
        &self,
        project_id: EntityId,
        environment: Environment,
        revision: u64,
        notify: bool,
    ) -> Result<(), StorageQueueError> {
        let command = StorageCommand::SaveEnvironment {
            project_id,
            environment,
            revision,
            notify,
        };
        self.commands
            .as_ref()
            .ok_or(StorageQueueError::Closed)?
            .try_send(command)
            .map_err(|error| match error {
                mpsc::TrySendError::Full(_) => StorageQueueError::Full,
                mpsc::TrySendError::Disconnected(_) => StorageQueueError::Closed,
            })
    }

    pub fn queue_import(
        &self,
        project_id: EntityId,
        requests: Vec<(ApiDefinition, RequestCase, u64)>,
    ) -> Result<(), StorageQueueError> {
        self.commands
            .as_ref()
            .ok_or(StorageQueueError::Closed)?
            .try_send(StorageCommand::ImportRequests {
                project_id,
                requests,
            })
            .map_err(|error| match error {
                mpsc::TrySendError::Full(_) => StorageQueueError::Full,
                mpsc::TrySendError::Disconnected(_) => StorageQueueError::Closed,
            })
    }

    pub fn try_recv(&self) -> Option<StorageEvent> {
        self.events.try_recv().ok()
    }

    pub fn flush(&self) -> Result<(), StorageQueueError> {
        let (acknowledged, receiver) = mpsc::channel();
        self.commands
            .as_ref()
            .ok_or(StorageQueueError::Closed)?
            .send(StorageCommand::Flush { acknowledged })
            .map_err(|_| StorageQueueError::Closed)?;
        receiver.recv().map_err(|_| StorageQueueError::Closed)
    }

    #[cfg(test)]
    pub fn recv_timeout(&self, timeout: Duration) -> Option<StorageEvent> {
        self.events.recv_timeout(timeout).ok()
    }
}

impl Drop for StorageWorker {
    fn drop(&mut self) {
        self.commands.take();
        if let Some(task) = self.task.take() {
            let _ = task.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{sync::Arc, time::Duration};

    use apitest_core::{
        ApiDefinition, Environment, HttpMethod, HttpSpec, Project, ProtocolSpec, RequestCase,
    };
    use apitest_storage::Database;

    use super::{StorageEvent, StorageWorker, open_workspace_database};

    #[test]
    fn first_v2_start_archives_v1_without_importing_or_deleting_it() {
        let temp = tempfile::tempdir().expect("temporary directory should exist");
        let legacy_path = temp.path().join("apitest.sqlite3");
        let legacy = Database::open(&legacy_path).expect("legacy database should open");
        legacy
            .save_project(&Project::new("Legacy"))
            .expect("legacy project should save");

        let startup =
            open_workspace_database(temp.path()).expect("v2 workspace database should open");

        assert_eq!(
            startup.database.path(),
            Some(temp.path().join("apitest-v2.sqlite3").as_path())
        );
        assert!(startup.legacy_archive.is_some_and(|path| path.exists()));
        assert!(legacy_path.exists());
        assert!(
            startup
                .database
                .list_projects()
                .expect("v2 projects should load")
                .is_empty()
        );
    }

    #[test]
    fn storage_worker_acknowledges_the_exact_saved_revision() {
        let database = Arc::new(Database::open_in_memory().expect("database should open"));
        let project = Project::new("Autosave");
        let definition = ApiDefinition::new(
            "List users",
            ProtocolSpec::Http(HttpSpec::new(HttpMethod::Get, "https://example.test/users")),
        );
        let mut request_case = RequestCase::for_definition(&definition, "Default");
        request_case.name = "Edited".into();
        database
            .save_project(&project)
            .expect("project should save");
        database
            .save_definition(project.id, &definition)
            .expect("definition should save");
        let worker = StorageWorker::new(Arc::clone(&database));

        worker
            .queue_request(
                project.id,
                definition.clone(),
                request_case.clone(),
                7,
                false,
            )
            .expect("save should queue");
        let event = worker
            .recv_timeout(Duration::from_secs(1))
            .expect("worker should acknowledge");

        assert!(matches!(
            event,
            StorageEvent::RequestSaved {
                revision: 7,
                definition: saved_definition,
                request_case: saved_case,
                notify: false,
            } if saved_definition == definition && saved_case == request_case
        ));
        assert_eq!(
            database
                .list_request_cases(
                    project.id,
                    definition.id,
                    apitest_storage::PageRequest::new(0, 10),
                )
                .expect("cases should load")
                .items,
            vec![request_case]
        );
    }

    #[test]
    fn storage_worker_saves_environments_without_blocking_the_ui_thread() {
        let database = Arc::new(Database::open_in_memory().expect("database should open"));
        let project = Project::new("Autosave");
        let environment = Environment::new("Staging");
        database
            .save_project(&project)
            .expect("project should save");
        let worker = StorageWorker::new(Arc::clone(&database));

        worker
            .queue_environment(project.id, environment.clone(), 3, false)
            .expect("save should queue");
        let event = worker
            .recv_timeout(Duration::from_secs(1))
            .expect("worker should acknowledge");

        assert!(matches!(
            event,
            StorageEvent::EnvironmentSaved {
                revision: 3,
                environment: saved,
                notify: false,
            } if saved == environment
        ));
        assert_eq!(
            database
                .list_environments(project.id)
                .expect("environments should load"),
            vec![environment]
        );
    }

    #[test]
    fn bulk_import_uses_one_queue_slot_and_acknowledges_every_request() {
        let database = Arc::new(Database::open_in_memory().expect("database should open"));
        let project = Project::new("Bulk import");
        database
            .save_project(&project)
            .expect("project should save");
        let requests = (0..70)
            .map(|index| {
                let definition = ApiDefinition::new(
                    format!("Request {index}"),
                    ProtocolSpec::Http(HttpSpec::new(
                        HttpMethod::Get,
                        format!("https://example.test/{index}"),
                    )),
                );
                let request_case = RequestCase::for_definition(&definition, "Default");
                (definition, request_case, index + 1)
            })
            .collect::<Vec<_>>();
        let worker = StorageWorker::new(Arc::clone(&database));

        worker
            .queue_import(project.id, requests)
            .expect("bulk import should occupy one queue slot");
        for revision in 1..=70 {
            let event = worker
                .recv_timeout(Duration::from_secs(1))
                .expect("every imported request should be acknowledged");
            assert!(matches!(
                event,
                StorageEvent::RequestSaved {
                    revision: saved_revision,
                    notify: false,
                    ..
                } if saved_revision == revision
            ));
        }
        assert_eq!(
            database
                .list_definitions(project.id)
                .expect("definitions should load")
                .len(),
            70
        );
    }

    #[test]
    fn storage_worker_flush_waits_for_preceding_writes() {
        let database = Arc::new(Database::open_in_memory().expect("database should open"));
        let project = Project::new("Flush");
        let definition = ApiDefinition::new(
            "Queued",
            ProtocolSpec::Http(HttpSpec::new(
                HttpMethod::Get,
                "https://example.test/queued",
            )),
        );
        let request_case = RequestCase::for_definition(&definition, "Default");
        database
            .save_project(&project)
            .expect("project should save");
        let worker = StorageWorker::new(Arc::clone(&database));

        worker
            .queue_request(project.id, definition.clone(), request_case, 1, false)
            .expect("save should queue");
        worker.flush().expect("flush should wait for the save");

        assert_eq!(
            database
                .get_definition(definition.id)
                .expect("definition should load"),
            Some(definition)
        );
    }
}
