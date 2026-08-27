use std::io::Write;

use apitest_core::{EntityId, ExecutionMetrics, RunRecord, RunState as HistoryRunState};
use apitest_storage::{BodyRef, PageRequest};
use chrono::Utc;

use crate::app::ApiTestApp;
use crate::draft::BodyMode;
use crate::i18n::Language;
use crate::services::secrets::sensitive_name;
use crate::state::action::ToastKind;
use crate::state::response::MAX_RESPONSE_BYTES;
use crate::ui::text_view::split_display_rows;
use crate::workbench::DocumentId;

pub(crate) const HISTORY_MAX_RECORDS: usize = 200;

pub(crate) const HISTORY_MAX_AGE_DAYS: i64 = 30;

impl ApiTestApp {
    pub(crate) fn history_redaction_values(
        &self,
        request_index: usize,
        environment_index: usize,
    ) -> Vec<String> {
        let mut values = Vec::new();
        let request = &self.requests[request_index];
        for secret in request.draft.auth.secrets() {
            if let Ok(Some(value)) = self.secrets.get(&secret.reference) {
                values.push(value);
            }
        }
        for variable in &request.request_case.local_variables {
            if let Some(reference) = &variable.secret_ref
                && let Ok(Some(value)) = self.secrets.get(reference)
            {
                values.push(value);
            }
        }
        if let Some(proxy) = &request.draft.proxy
            && let Ok(Some(value)) = self.secrets.get(&proxy.password.reference)
        {
            values.push(value);
        }
        for variable in &self.environments[environment_index].variables {
            if variable.secret {
                if let Some(reference) = &variable.reference
                    && let Ok(Some(value)) = self.secrets.get(reference)
                {
                    values.push(value);
                }
            } else if sensitive_name(&variable.name) && !variable.value.is_empty() {
                values.push(variable.value.clone());
            }
        }
        for pair in request
            .draft
            .query
            .iter()
            .chain(&request.draft.headers)
            .chain(&request.draft.cookies)
            .chain(&request.draft.form_fields)
        {
            if pair.enabled && sensitive_name(&pair.name) && !pair.value.is_empty() {
                values.push(pair.value.clone());
                if let Some((_, credential)) = pair.value.split_once(' ')
                    && !credential.is_empty()
                {
                    values.push(credential.to_owned());
                }
            }
        }
        for part in &request.draft.multipart_parts {
            if part.enabled && sensitive_name(&part.name) && !part.value.is_empty() {
                values.push(part.value.clone());
            }
        }
        if request.draft.body_mode == BodyMode::Json
            && let Ok(body) = serde_json::from_str::<serde_json::Value>(&request.draft.body)
        {
            collect_sensitive_json_values(&body, &mut values);
        }
        values.retain(|value| !value.is_empty());
        values.sort_by_key(|value| std::cmp::Reverse(value.len()));
        values.dedup();
        values
    }

    pub(crate) fn begin_run_history(
        &mut self,
        document: DocumentId,
        request_case_id: EntityId,
        redactions: Vec<String>,
    ) {
        let sink = match self.body_store.as_ref() {
            Some(store) => match store.begin_redacted(&redactions) {
                Ok(sink) => Some(sink),
                Err(error) => {
                    self.toast(ToastKind::Error, error.to_string());
                    None
                }
            },
            None => None,
        };
        let session = self.sessions.entry(document);
        session.history_record = Some(RunRecord {
            id: EntityId::new(),
            request_case_id: Some(request_case_id),
            state: HistoryRunState::Running,
            status_code: None,
            elapsed_ms: 0,
            response_bytes: 0,
            body_path: None,
            error: None,
            started_at: Utc::now(),
            finished_at: None,
        });
        session.history_redactions = redactions;
        session.history_body = sink;
    }

    pub(crate) fn write_run_history_body(&mut self, document: DocumentId, bytes: &[u8]) {
        let session = self.sessions.entry(document);
        let error = session
            .history_body
            .as_mut()
            .and_then(|sink| sink.write_all(bytes).err());
        if let Some(error) = error {
            session.history_body = None;
            self.toast(
                ToastKind::Error,
                match self.language {
                    Language::Chinese => format!("响应体写入失败：{error}"),
                    Language::English => format!("failed to store response body: {error}"),
                },
            );
        }
    }

    pub(crate) fn finish_run_history(
        &mut self,
        document: DocumentId,
        state: HistoryRunState,
        metrics: Option<ExecutionMetrics>,
        error: Option<String>,
    ) {
        let session = self.sessions.entry(document);
        let status_code = session.response.status;
        let redactions = std::mem::take(&mut session.history_redactions);
        let Some(mut record) = session.history_record.take() else {
            session.history_body = None;
            return;
        };
        let body = match session.history_body.take() {
            Some(sink) => match sink.commit() {
                Ok(body) => Some(body),
                Err(storage_error) => {
                    self.toast(ToastKind::Error, storage_error.to_string());
                    None
                }
            },
            None => None,
        };
        let finished_at = Utc::now();
        record.state = state;
        record.status_code = status_code;
        record.elapsed_ms = metrics
            .map(|metrics| metrics.elapsed_ms)
            .unwrap_or_else(|| (finished_at - record.started_at).num_milliseconds().max(0) as u64);
        record.response_bytes = metrics
            .map(|metrics| metrics.received_bytes)
            .or_else(|| body.as_ref().map(|body| body.size))
            .unwrap_or_default();
        record.body_path = body.as_ref().map(|body| body.path.display().to_string());
        record.error = error.map(|error| redact_text(&error, &redactions));
        record.finished_at = Some(finished_at);

        let Some(database) = self.database.clone() else {
            if let (Some(store), Some(body)) = (&self.body_store, body.as_ref()) {
                let _ = store.delete(body);
            }
            return;
        };
        if let Err(storage_error) = database.save_run_record(self.project.id, &record) {
            if let (Some(store), Some(body)) = (&self.body_store, body.as_ref()) {
                let _ = store.delete(body);
            }
            self.toast(ToastKind::Error, storage_error.to_string());
            return;
        }
        match database.prune_run_records_with_body_paths(
            self.project.id,
            HISTORY_MAX_RECORDS,
            HISTORY_MAX_AGE_DAYS,
            finished_at,
        ) {
            Ok((_, paths)) => {
                if let Some(store) = &self.body_store {
                    for path in paths {
                        if let Err(storage_error) = store.delete(&BodyRef { path, size: 0 }) {
                            tracing::warn!(%storage_error, "failed to delete expired response body");
                        }
                    }
                }
            }
            Err(storage_error) => self.toast(ToastKind::Error, storage_error.to_string()),
        }
        self.reload_run_history();
    }

    pub(crate) fn reload_run_history(&mut self) {
        let Some(database) = self.database.clone() else {
            self.run_records.clear();
            self.history_body_preview.clear();
            self.history_preview_rows.clear();
            self.history_body_truncated = false;
            return;
        };
        let selected_id = self
            .run_records
            .get(self.selected_history)
            .map(|record| record.id);
        match database.list_run_records(self.project.id, PageRequest::new(0, HISTORY_MAX_RECORDS)) {
            Ok(page) => {
                self.run_records = page.items;
                self.selected_history = selected_id
                    .and_then(|id| self.run_records.iter().position(|record| record.id == id))
                    .unwrap_or_default();
                self.load_selected_history_body();
            }
            Err(error) => self.toast(ToastKind::Error, error.to_string()),
        }
    }

    pub(crate) fn select_run_history(&mut self, index: usize) {
        if self.run_records.get(index).is_none() {
            return;
        }
        self.selected_history = index;
        self.load_selected_history_body();
    }

    pub(crate) fn load_selected_history_body(&mut self) {
        self.fill_history_preview();
        self.history_preview_rows = split_display_rows(&self.history_body_preview);
    }

    fn fill_history_preview(&mut self) {
        self.history_body_preview.clear();
        self.history_body_truncated = false;
        let Some(path) = self
            .run_records
            .get(self.selected_history)
            .and_then(|record| record.body_path.as_deref())
        else {
            return;
        };
        let Some(store) = &self.body_store else {
            self.history_body_preview = self
                .tr("响应体存储不可用", "response body store is unavailable")
                .into();
            return;
        };
        let path = std::path::PathBuf::from(path);
        let size = match std::fs::metadata(&path) {
            Ok(metadata) => metadata.len(),
            Err(error) => {
                self.history_body_preview = match self.language {
                    Language::Chinese => format!("读取响应体信息失败：{error}"),
                    Language::English => format!("failed to inspect response body: {error}"),
                };
                return;
            }
        };
        match store.read_range(&BodyRef { path, size }, 0, MAX_RESPONSE_BYTES) {
            Ok(bytes) => {
                self.history_body_preview = String::from_utf8_lossy(&bytes).into_owned();
                self.history_body_truncated = size > bytes.len() as u64;
            }
            Err(error) => {
                self.history_body_preview = match self.language {
                    Language::Chinese => format!("读取响应体失败：{error}"),
                    Language::English => format!("failed to read response body: {error}"),
                };
            }
        }
    }
}

pub(crate) fn collect_sensitive_json_values(value: &serde_json::Value, values: &mut Vec<String>) {
    match value {
        serde_json::Value::Object(object) => {
            for (name, value) in object {
                if sensitive_name(name) {
                    collect_json_strings(value, values);
                } else {
                    collect_sensitive_json_values(value, values);
                }
            }
        }
        serde_json::Value::Array(items) => {
            for item in items {
                collect_sensitive_json_values(item, values);
            }
        }
        _ => {}
    }
}

pub(crate) fn collect_json_strings(value: &serde_json::Value, values: &mut Vec<String>) {
    match value {
        serde_json::Value::String(value) => values.push(value.clone()),
        serde_json::Value::Array(items) => {
            for item in items {
                collect_json_strings(item, values);
            }
        }
        serde_json::Value::Object(object) => {
            for value in object.values() {
                collect_json_strings(value, values);
            }
        }
        _ => {}
    }
}

pub(crate) fn redact_text(value: &str, redactions: &[String]) -> String {
    redactions.iter().fold(value.to_owned(), |value, secret| {
        value.replace(secret, "[REDACTED]")
    })
}
