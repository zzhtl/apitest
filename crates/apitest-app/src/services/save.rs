use std::time::{Duration, Instant};

use apitest_core::SecretRef;
use eframe::egui::{self};

use crate::app::ApiTestApp;
use crate::environment::EnvironmentDraft;
use crate::services::document::document_snapshot;
use crate::services::secrets::persist_request_draft_secrets;
use crate::state::action::ToastKind;
use crate::state::workspace::{Navigation, WorkspaceRequest};

impl ApiTestApp {
    pub(crate) fn save_current(&mut self) -> bool {
        match self.navigation {
            Navigation::Api => self.save_current_request(),
            Navigation::Environment => self.save_current_environment(),
            Navigation::Scenario => self.save_current_scenario(),
            Navigation::Mock => self.save_current_mock(),
            Navigation::History => true,
        }
    }

    pub(crate) fn save_current_and_wait(&mut self) -> bool {
        if !self.save_current() {
            return false;
        }
        match self.navigation {
            Navigation::Api | Navigation::Environment => {
                self.flush_storage() && !self.current_dirty()
            }
            Navigation::Scenario | Navigation::Mock | Navigation::History => !self.current_dirty(),
        }
    }

    pub(crate) fn save_workspace_and_wait(&mut self) -> bool {
        for index in 0..self.requests.len() {
            if self.requests[index].is_dirty()
                && (!self.queue_request_save(index, true, false) || !self.flush_storage())
            {
                return false;
            }
        }
        for index in 0..self.environments.len() {
            if self.environments[index].is_dirty()
                && (!self.queue_environment_save(index, true, false) || !self.flush_storage())
            {
                return false;
            }
        }
        let selected_scenario = self.selected_scenario;
        for index in 0..self.scenarios.len() {
            let dirty = self
                .scenario_snapshots
                .get(&self.scenarios[index].id)
                .is_none_or(|saved| saved != &document_snapshot(&self.scenarios[index]));
            if dirty {
                self.selected_scenario = index;
                if !self.save_current_scenario() {
                    self.selected_scenario = selected_scenario;
                    return false;
                }
            }
        }
        self.selected_scenario = selected_scenario.min(self.scenarios.len().saturating_sub(1));
        let selected_mock = self.selected_mock;
        for index in 0..self.mock_profiles.len() {
            let dirty = self
                .mock_snapshots
                .get(&self.mock_profiles[index].id)
                .is_none_or(|saved| saved != &document_snapshot(&self.mock_profiles[index]));
            if dirty {
                self.selected_mock = index;
                if !self.save_current_mock() {
                    self.selected_mock = selected_mock;
                    return false;
                }
            }
        }
        self.selected_mock = selected_mock.min(self.mock_profiles.len().saturating_sub(1));
        !self.workspace_dirty()
    }

    pub(crate) fn cleanup_secret_references<I>(&self, references: I)
    where
        I: IntoIterator<Item = SecretRef>,
    {
        for reference in references {
            if let Err(error) = self.secrets.delete(&reference) {
                tracing::warn!(%error, reference = reference.as_str(), "failed to clean unused secret");
            }
        }
    }

    pub(crate) fn persist_request_secret(&mut self, index: usize) -> Result<(), String> {
        let Some(request) = self.requests.get_mut(index) else {
            return Ok(());
        };
        persist_request_draft_secrets(self.secrets.as_ref(), &mut request.draft)
    }

    pub(crate) fn save_current_request(&mut self) -> bool {
        self.queue_request_save(self.selected, true, true)
    }

    pub(crate) fn queue_request_save(
        &mut self,
        index: usize,
        validate: bool,
        notify: bool,
    ) -> bool {
        if self.storage_worker.is_none() {
            // Only manual saves report the missing database: autosave retries every
            // frame and would otherwise queue an endless stream of error toasts.
            if notify {
                self.toast(
                    ToastKind::Error,
                    self.tr("本地数据库不可用", "Local database unavailable"),
                );
            }
            return false;
        }
        let Some(request) = self.requests.get(index) else {
            return false;
        };
        if validate && request.name.trim().is_empty() {
            self.toast(
                ToastKind::Error,
                self.tr("请求名称不能为空", "Request name cannot be empty"),
            );
            return false;
        }
        if validate {
            if let Err(error) = self.validate_auth(&request.draft.auth) {
                self.toast(ToastKind::Error, error);
                return false;
            }
            if let Err(error) = self.validate_proxy(request.draft.proxy.as_ref()) {
                self.toast(ToastKind::Error, error);
                return false;
            }
        }
        if let Err(error) = self.persist_request_secret(index) {
            // Re-arm the debounce instead of leaving the autosave due on the next
            // frame: a persistently failing keyring must not retry per frame.
            let revision = self.requests[index].autosave.current_revision();
            self.requests[index]
                .autosave
                .mark_failed(revision, Instant::now());
            if notify {
                self.toast(ToastKind::Error, error);
            }
            return false;
        }
        self.requests[index].sync_edit_revision(Instant::now());
        let revision = self.requests[index].autosave.current_revision();
        if revision == 0 && self.requests[index].persisted {
            if notify {
                self.toast(
                    ToastKind::Info,
                    self.tr("没有需要保存的更改", "No changes to save"),
                );
            }
            return true;
        }
        let (definition, request_case) = self.requests[index].save_snapshot();
        let result = self
            .storage_worker
            .as_ref()
            .expect("storage worker was checked above")
            .queue_request(self.project.id, definition, request_case, revision, notify);
        match result {
            Ok(()) => {
                self.requests[index].autosave.mark_queued(revision);
                true
            }
            Err(error) => {
                self.requests[index]
                    .autosave
                    .mark_failed(revision, Instant::now());
                self.toast(ToastKind::Error, error.to_string());
                false
            }
        }
    }

    pub(crate) fn schedule_request_autosaves(&mut self, context: &egui::Context) {
        if self.storage_worker.is_none() {
            return;
        }
        let now = Instant::now();
        for request in &mut self.requests {
            request.sync_edit_revision(now);
        }
        let due = self
            .requests
            .iter()
            .enumerate()
            .filter_map(|(index, request)| request.autosave.is_due(now).then_some(index))
            .collect::<Vec<_>>();
        for index in due {
            self.queue_request_save(index, false, false);
        }
        if self.requests.iter().any(WorkspaceRequest::is_dirty) {
            context.request_repaint_after(Duration::from_millis(250));
        }
    }

    pub(crate) fn persist_environment_secrets(&mut self, index: usize) -> Result<(), String> {
        let environment_id = self.environments[index].id();
        let mut pending = Vec::new();
        for (variable_index, variable) in self.environments[index].variables.iter_mut().enumerate()
        {
            if variable.has_pending_secret() {
                let reference = variable.ensure_reference(environment_id).clone();
                pending.push((variable_index, reference, variable.value.clone()));
            }
        }
        for (_, reference, value) in &pending {
            self.secrets
                .set(reference, value)
                .map_err(|error| error.to_string())?;
        }
        for (variable_index, _, _) in pending {
            self.environments[index].variables[variable_index].mark_secret_saved();
        }
        Ok(())
    }

    pub(crate) fn save_current_environment(&mut self) -> bool {
        self.queue_environment_save(self.selected_environment, true, true)
    }

    pub(crate) fn queue_environment_save(
        &mut self,
        index: usize,
        validate: bool,
        notify: bool,
    ) -> bool {
        if self.storage_worker.is_none() {
            if notify {
                self.toast(
                    ToastKind::Error,
                    self.tr("本地数据库不可用", "Local database unavailable"),
                );
            }
            return false;
        }
        if self.environments.get(index).is_none() {
            return false;
        }
        if let Err(error) = self.validate_environment(index) {
            let revision = self.environments[index].autosave.current_revision();
            self.environments[index]
                .autosave
                .mark_failed(revision, Instant::now());
            if validate {
                self.toast(ToastKind::Error, error);
            }
            return false;
        }
        if let Err(error) = self.persist_environment_secrets(index) {
            let revision = self.environments[index].autosave.current_revision();
            self.environments[index]
                .autosave
                .mark_failed(revision, Instant::now());
            if notify {
                self.toast(ToastKind::Error, error);
            }
            return false;
        }
        self.environments[index].sync_edit_revision(Instant::now());
        let revision = self.environments[index].autosave.current_revision();
        if revision == 0 && self.environments[index].persisted {
            if notify {
                self.toast(
                    ToastKind::Info,
                    self.tr("没有需要保存的更改", "No changes to save"),
                );
            }
            return true;
        }
        let environment = self.environments[index].to_environment();
        let result = self
            .storage_worker
            .as_ref()
            .expect("storage worker was checked above")
            .queue_environment(self.project.id, environment, revision, notify);
        match result {
            Ok(()) => {
                self.environments[index].autosave.mark_queued(revision);
                true
            }
            Err(error) => {
                self.environments[index]
                    .autosave
                    .mark_failed(revision, Instant::now());
                self.toast(ToastKind::Error, error.to_string());
                false
            }
        }
    }

    pub(crate) fn schedule_environment_autosaves(&mut self, context: &egui::Context) {
        if self.storage_worker.is_none() {
            return;
        }
        let now = Instant::now();
        for environment in &mut self.environments {
            environment.sync_edit_revision(now);
        }
        let due = self
            .environments
            .iter()
            .enumerate()
            .filter_map(|(index, environment)| environment.autosave.is_due(now).then_some(index))
            .collect::<Vec<_>>();
        for index in due {
            self.queue_environment_save(index, false, false);
        }
        if self.environments.iter().any(EnvironmentDraft::is_dirty) {
            context.request_repaint_after(Duration::from_millis(250));
        }
    }
}
