use apitest_core::{EntityId, MockProfile, generate_mock_rules};
use apitest_runtime::{MockRoute, MockServer};
use eframe::egui::{self};

use crate::app::ApiTestApp;
use crate::i18n::Language;
use crate::services::document::document_snapshot;
use crate::state::action::{RuntimeMessage, ToastKind};
use crate::workbench::{DocumentId, DocumentKind};

impl ApiTestApp {
    pub(crate) fn save_current_mock(&mut self) -> bool {
        let Some(profile) = self.mock_profiles.get(self.selected_mock) else {
            return false;
        };
        if let Err(error) = validate_mock_profile(profile) {
            self.toast(ToastKind::Error, error);
            return false;
        }
        let Some(database) = &self.database else {
            self.toast(
                ToastKind::Error,
                self.tr("本地数据库不可用", "Local database unavailable"),
            );
            return false;
        };
        if let Err(error) = database.save_mock_profile(self.project.id, profile) {
            self.toast(ToastKind::Error, error.to_string());
            return false;
        }
        let id = profile.id;
        let name = profile.name.clone();
        self.mock_snapshots.insert(id, document_snapshot(profile));
        self.document_tabs.rename(
            DocumentId {
                kind: DocumentKind::Mock,
                entity_id: id,
            },
            name,
        );
        self.persist_document_tabs();
        self.toast(ToastKind::Success, self.tr("Mock 已保存", "Mock saved"));
        true
    }

    pub(crate) fn generate_current_mock_rules(&mut self) {
        let definitions = self.contract_definitions();
        let rules = generate_mock_rules(&definitions, &self.project.components);
        if rules.is_empty() {
            self.toast(
                ToastKind::Error,
                self.tr(
                    "当前项目没有可生成 Mock 的 HTTP 契约",
                    "The project has no HTTP contracts to mock",
                ),
            );
            return;
        }
        let mut count = 0;
        if let Some(profile) = self.mock_profiles.get_mut(self.selected_mock) {
            for rule in rules {
                if profile
                    .rules
                    .iter()
                    .any(|existing| existing.method == rule.method && existing.path == rule.path)
                {
                    continue;
                }
                profile.rules.push(rule);
                count += 1;
            }
        }
        self.toast(
            ToastKind::Info,
            match self.language {
                Language::Chinese => format!("已根据契约新增 {count} 条 Mock 规则"),
                Language::English => format!("Added {count} mock rules from contracts"),
            },
        );
    }

    pub(crate) fn start_current_mock(&mut self, context: &egui::Context) {
        if self.mock_server.is_some() {
            return;
        }
        let Some(profile) = self.mock_profiles.get(self.selected_mock) else {
            return;
        };
        if let Err(error) = validate_mock_profile(profile) {
            self.toast(ToastKind::Error, error);
            return;
        }
        let address = match profile.bind_address.parse::<std::net::IpAddr>() {
            Ok(address) => std::net::SocketAddr::new(address, profile.port),
            Err(error) => {
                self.toast(ToastKind::Error, error.to_string());
                return;
            }
        };
        let routes = profile
            .rules
            .iter()
            .filter(|rule| rule.enabled)
            .map(MockRoute::from)
            .collect::<Vec<_>>();
        self.mock_run_id = self.mock_run_id.wrapping_add(1);
        let run_id = self.mock_run_id;
        let sender = self.sender.clone();
        let context = context.clone();
        self.runtime.spawn(async move {
            let result = MockServer::start(address, routes)
                .await
                .map_err(|error| error.to_string());
            let _ = sender.send(RuntimeMessage::MockStarted(run_id, result));
            context.request_repaint();
        });
    }

    pub(crate) fn delete_mock(&mut self, id: EntityId) {
        let Some(database) = self.database.clone() else {
            self.toast(
                ToastKind::Error,
                self.tr("本地数据库不可用", "Local database unavailable"),
            );
            return;
        };
        if let Err(error) = database.delete_mock_profile(self.project.id, id) {
            self.toast(ToastKind::Error, error.to_string());
            return;
        }
        // Stop the server before the profile that describes it disappears.
        if self
            .mock_profiles
            .get(self.selected_mock)
            .is_some_and(|profile| profile.id == id)
        {
            self.stop_current_mock();
        }
        self.mock_snapshots.remove(&id);
        if let Some(index) = self
            .mock_profiles
            .iter()
            .position(|profile| profile.id == id)
        {
            self.mock_profiles.remove(index);
        }
        self.selected_mock = self
            .selected_mock
            .min(self.mock_profiles.len().saturating_sub(1));
        self.close_document(DocumentId {
            kind: DocumentKind::Mock,
            entity_id: id,
        });
        self.toast(ToastKind::Success, self.tr("Mock 已删除", "Mock deleted"));
    }

    pub(crate) fn stop_current_mock(&mut self) {
        self.mock_run_id = self.mock_run_id.wrapping_add(1);
        if let Some(server) = self.mock_server.take() {
            self.runtime.spawn(server.shutdown());
            self.toast(
                ToastKind::Info,
                self.tr("Mock 服务已停止", "Mock server stopped"),
            );
        }
    }
}

pub(crate) fn validate_mock_profile(profile: &MockProfile) -> Result<(), String> {
    if profile.name.trim().is_empty() {
        return Err("mock profile name cannot be empty".into());
    }
    profile
        .bind_address
        .parse::<std::net::IpAddr>()
        .map_err(|error| format!("invalid mock bind address: {error}"))?;
    for rule in &profile.rules {
        if rule.name.trim().is_empty() {
            return Err("mock rule name cannot be empty".into());
        }
        if !rule.path.starts_with('/') {
            return Err(format!(
                "mock rule `{}` path must start with `/`",
                rule.name
            ));
        }
        if !(100..=599).contains(&rule.response.status) {
            return Err(format!("mock rule `{}` has an invalid status", rule.name));
        }
    }
    Ok(())
}
