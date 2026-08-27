use std::{
    collections::HashSet,
    time::{Duration, Instant},
};

use apitest_core::SecretRef;

use crate::app::ApiTestApp;
use apitest_storage::BackupManager;

use crate::persistence::{StorageEvent, StorageWorker};
use crate::state::action::ToastKind;
use crate::workbench::{DocumentId, DocumentKind};

/// Minimum pause between cookie-jar saves; runs finishing back-to-back
/// coalesce into one write.
const COOKIE_SAVE_INTERVAL: Duration = Duration::from_secs(5);

/// How often the database file is snapshotted while the app runs.
const BACKUP_INTERVAL: Duration = Duration::from_secs(30 * 60);

/// How many rolling snapshots are kept.
const BACKUP_RETENTION: usize = 5;

impl ApiTestApp {
    /// Queue a rolling backup once at startup and every [`BACKUP_INTERVAL`].
    ///
    /// Runs on the storage worker from the file path, so nothing blocks the
    /// app's own connection while pages are copied.
    pub(crate) fn schedule_backups(&mut self) {
        if self
            .last_backup
            .is_some_and(|last| last.elapsed() < BACKUP_INTERVAL)
        {
            return;
        }
        let Some(worker) = &self.storage_worker else {
            return;
        };
        let Some(source) = self
            .database
            .as_deref()
            .and_then(|database| database.path())
            .map(std::path::Path::to_path_buf)
        else {
            return;
        };
        let Some(directory) = source.parent().map(|parent| parent.join("backups")) else {
            return;
        };
        self.last_backup = Some(Instant::now());
        match BackupManager::new(directory, BACKUP_RETENTION) {
            Ok(manager) => {
                if let Err(error) = worker.queue_backup(manager, source) {
                    tracing::warn!(%error, "failed to queue a rolling backup");
                }
            }
            Err(error) => tracing::warn!(%error, "failed to prepare the backup directory"),
        }
    }

    pub(crate) fn save_cookie_jar_if_due(&mut self) {
        if !self.cookies_dirty {
            return;
        }
        if self
            .last_cookie_save
            .is_some_and(|last| last.elapsed() < COOKIE_SAVE_INTERVAL)
        {
            return;
        }
        self.save_cookie_jar();
    }

    /// Persist the jar's non-session cookies into settings.
    pub(crate) fn save_cookie_jar(&mut self) {
        if !self.cookies_dirty {
            return;
        }
        let Some(json) = self.cookie_jar.to_json() else {
            return;
        };
        self.cookies_dirty = false;
        self.last_cookie_save = Some(Instant::now());
        self.persist_setting(crate::app::COOKIE_JAR_SETTING, &json);
    }

    pub(crate) fn drain_storage(&mut self) -> bool {
        self.drain_storage_protected(&HashSet::new())
    }

    pub(crate) fn drain_storage_protected(
        &mut self,
        protected_environment_secrets: &HashSet<SecretRef>,
    ) -> bool {
        let mut succeeded = true;
        loop {
            let Some(event) = self
                .storage_worker
                .as_ref()
                .and_then(StorageWorker::try_recv)
            else {
                break;
            };
            match event {
                StorageEvent::RequestSaved {
                    revision,
                    definition,
                    request_case,
                    notify,
                } => {
                    let mut renamed = None;
                    if let Some(request) = self
                        .requests
                        .iter_mut()
                        .find(|request| request.id() == definition.id)
                    {
                        request.mark_saved(definition, request_case, revision);
                        renamed = Some((request.id(), request.name.clone()));
                    }
                    // The FTS index just changed under the cached results.
                    self.invalidate_search_cache();
                    if let Some((id, name)) = renamed {
                        self.document_tabs.rename(
                            DocumentId {
                                kind: DocumentKind::Api,
                                entity_id: id,
                            },
                            name,
                        );
                        self.persist_document_tabs();
                        self.reload_resource_page(None);
                    }
                    if notify {
                        self.toast(ToastKind::Success, self.tr("已保存", "Saved"));
                    }
                }
                StorageEvent::EnvironmentSaved {
                    revision,
                    environment,
                    notify,
                } => {
                    let mut renamed = None;
                    if let Some(draft) = self
                        .environments
                        .iter_mut()
                        .find(|draft| draft.id() == environment.id)
                    {
                        let old_references = draft
                            .saved
                            .variables
                            .iter()
                            .filter_map(|variable| variable.secret_ref.clone())
                            .collect::<HashSet<_>>();
                        let new_references = environment
                            .variables
                            .iter()
                            .filter_map(|variable| variable.secret_ref.clone())
                            .collect::<HashSet<_>>();
                        let current_references = draft
                            .variables
                            .iter()
                            .filter(|variable| variable.secret)
                            .filter_map(|variable| variable.reference.clone())
                            .collect::<HashSet<_>>();
                        for reference in old_references.difference(&new_references) {
                            if current_references.contains(reference)
                                || protected_environment_secrets.contains(reference)
                            {
                                continue;
                            }
                            if let Err(error) = self.secrets.delete(reference) {
                                tracing::warn!(%error, "failed to clean unused environment secret");
                            }
                        }
                        draft.mark_saved(environment, revision);
                        renamed = Some((draft.id(), draft.name.clone()));
                    }
                    if let Some((id, name)) = renamed {
                        self.document_tabs.rename(
                            DocumentId {
                                kind: DocumentKind::Environment,
                                entity_id: id,
                            },
                            name,
                        );
                        self.persist_document_tabs();
                    }
                    if notify {
                        self.toast(
                            ToastKind::Success,
                            self.tr("环境已保存", "Environment saved"),
                        );
                    }
                }
                StorageEvent::SaveFailed {
                    revision,
                    entity_id,
                    error,
                } => {
                    succeeded = false;
                    if let Some(request) = self
                        .requests
                        .iter_mut()
                        .find(|request| request.id() == entity_id)
                    {
                        request.autosave.mark_failed(revision, Instant::now());
                    } else if let Some(environment) = self
                        .environments
                        .iter_mut()
                        .find(|environment| environment.id() == entity_id)
                    {
                        environment.autosave.mark_failed(revision, Instant::now());
                    }
                    self.toast(ToastKind::Error, error);
                }
                StorageEvent::RunFinished {
                    error,
                    prune_failures,
                } => {
                    if let Some(error) = error {
                        succeeded = false;
                        self.toast(ToastKind::Error, error);
                    }
                    if prune_failures > 0 {
                        tracing::warn!(prune_failures, "failed to delete expired response bodies");
                    }
                    self.reload_run_history();
                }
            }
        }
        succeeded
    }

    pub(crate) fn wait_storage(&mut self) -> bool {
        let result = self
            .storage_worker
            .as_ref()
            .map(StorageWorker::flush)
            .transpose();
        if let Err(error) = result {
            self.toast(ToastKind::Error, error.to_string());
            return false;
        }
        true
    }

    pub(crate) fn flush_storage(&mut self) -> bool {
        self.wait_storage() && self.drain_storage()
    }

    pub(crate) fn settle_storage(&mut self) -> bool {
        if !self.wait_storage() {
            return false;
        }
        let _ = self.drain_storage();
        true
    }
}
