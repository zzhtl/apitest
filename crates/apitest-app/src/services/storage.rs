use std::{collections::HashSet, time::Instant};

use apitest_core::SecretRef;

use crate::app::ApiTestApp;
use crate::persistence::{StorageEvent, StorageWorker};
use crate::state::action::ToastKind;
use crate::workbench::{DocumentId, DocumentKind};

impl ApiTestApp {
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
