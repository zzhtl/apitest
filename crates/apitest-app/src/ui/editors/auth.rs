use std::sync::Arc;

use apitest_core::{ApiKeyLocation, EntityId, OAuth2Grant};
use apitest_storage::SecretStore;
use eframe::egui::{self, RichText};

use crate::draft::{AuthDraft, AuthMode};
use crate::i18n::{Language, tr};
use crate::theme::UiExt;
use crate::ui::widgets::empty_state;

pub(crate) fn auth_editor(
    ui: &mut egui::Ui,
    auth: &mut AuthDraft,
    request_id: EntityId,
    secrets: Arc<dyn SecretStore>,
    language: Language,
) -> Option<String> {
    let palette = ui.palette();
    let mut mode = auth.mode();
    let previous = mode;
    ui.horizontal_wrapped(|ui| {
        for (value, chinese, english) in [
            (AuthMode::None, "无认证", "None"),
            (AuthMode::Basic, "Basic", "Basic"),
            (AuthMode::Bearer, "Bearer", "Bearer"),
            (AuthMode::ApiKey, "API Key", "API Key"),
            (AuthMode::OAuth2, "OAuth 2.0", "OAuth 2.0"),
            (AuthMode::Digest, "Digest", "Digest"),
            (AuthMode::AwsSigV4, "AWS SigV4", "AWS SigV4"),
        ] {
            ui.selectable_value(
                &mut mode,
                value,
                match language {
                    Language::Chinese => chinese,
                    Language::English => english,
                },
            );
        }
    });
    let mut error = None;
    if mode != previous {
        *auth = AuthDraft::for_mode(mode, request_id);
        for secret in auth.secrets_mut() {
            match secrets.get(&secret.reference) {
                Ok(Some(_)) => secret.configured = true,
                Ok(None) => {}
                Err(value) => error = Some(value.to_string()),
            }
        }
    }
    ui.add_space(8.0);
    match auth {
        AuthDraft::None => empty_state(ui, tr(language, "无认证", "No authentication"), ""),
        AuthDraft::Basic { username, password } => {
            form_field(ui, language, "用户名", "Username", |ui| {
                ui.add_sized([360.0, 32.0], egui::TextEdit::singleline(username));
            });
            form_field(ui, language, "密码", "Password", |ui| {
                ui.add_sized(
                    [360.0, 32.0],
                    egui::TextEdit::singleline(&mut password.replacement)
                        .password(true)
                        .hint_text(if password.configured {
                            "••••••••"
                        } else {
                            ""
                        }),
                );
            });
        }
        AuthDraft::Bearer { token } => {
            form_field(ui, language, "Token", "Token", |ui| {
                ui.add_sized(
                    [520.0, 32.0],
                    egui::TextEdit::singleline(&mut token.replacement)
                        .password(true)
                        .hint_text(if token.configured {
                            "••••••••"
                        } else {
                            ""
                        }),
                );
            });
        }
        AuthDraft::ApiKey {
            name,
            value,
            location,
        } => {
            form_field(ui, language, "名称", "Name", |ui| {
                ui.add_sized([360.0, 32.0], egui::TextEdit::singleline(name));
            });
            form_field(ui, language, "值", "Value", |ui| {
                ui.add_sized(
                    [360.0, 32.0],
                    egui::TextEdit::singleline(&mut value.replacement)
                        .password(true)
                        .hint_text(if value.configured {
                            "••••••••"
                        } else {
                            ""
                        }),
                );
            });
            form_field(ui, language, "位置", "Location", |ui| {
                ui.selectable_value(location, ApiKeyLocation::Header, "Header");
                ui.selectable_value(location, ApiKeyLocation::Query, "Query");
            });
        }
        AuthDraft::OAuth2 {
            grant,
            authorization_url,
            token_url,
            client_id,
            client_secret,
            scopes,
            username,
            password,
            access_token,
        } => {
            form_field(ui, language, "授权类型", "Grant type", |ui| {
                egui::ComboBox::from_id_salt("oauth_grant")
                    .selected_text(match grant {
                        OAuth2Grant::ClientCredentials => "Client Credentials",
                        OAuth2Grant::Password => "Password",
                        OAuth2Grant::AuthorizationCodePkce => "Authorization Code + PKCE",
                    })
                    .show_ui(ui, |ui| {
                        ui.selectable_value(
                            grant,
                            OAuth2Grant::ClientCredentials,
                            "Client Credentials",
                        );
                        ui.selectable_value(grant, OAuth2Grant::Password, "Password");
                        ui.selectable_value(
                            grant,
                            OAuth2Grant::AuthorizationCodePkce,
                            "Authorization Code + PKCE",
                        );
                    });
            });
            if *grant == OAuth2Grant::AuthorizationCodePkce {
                form_field(ui, language, "授权地址", "Authorization URL", |ui| {
                    ui.add_sized(
                        [520.0, 32.0],
                        egui::TextEdit::singleline(authorization_url)
                            .hint_text("https://identity.example.com/authorize"),
                    );
                });
            }
            form_field(ui, language, "Token 地址", "Token URL", |ui| {
                ui.add_sized(
                    [520.0, 32.0],
                    egui::TextEdit::singleline(token_url)
                        .hint_text("https://identity.example.com/oauth/token"),
                );
            });
            form_field(ui, language, "Client ID", "Client ID", |ui| {
                ui.add_sized([360.0, 32.0], egui::TextEdit::singleline(client_id));
            });
            secret_form_field(
                ui,
                language,
                "Client 密钥",
                "Client secret",
                client_secret,
                360.0,
            );
            form_field(ui, language, "权限范围", "Scopes", |ui| {
                ui.add_sized(
                    [520.0, 32.0],
                    egui::TextEdit::singleline(scopes).hint_text("read write"),
                );
            });
            if *grant == OAuth2Grant::Password {
                form_field(ui, language, "用户名", "Username", |ui| {
                    ui.add_sized([360.0, 32.0], egui::TextEdit::singleline(username));
                });
                secret_form_field(ui, language, "密码", "Password", password, 360.0);
            }
            secret_form_field(
                ui,
                language,
                "访问令牌",
                "Access token",
                access_token,
                520.0,
            );
            ui.label(
                RichText::new(match language {
                    Language::Chinese => "访问令牌可选；配置后将跳过 Token 请求",
                    Language::English => {
                        "An access token is optional and bypasses the token request"
                    }
                })
                .small()
                .color(palette.muted),
            );
        }
        AuthDraft::Digest { username, password } => {
            form_field(ui, language, "用户名", "Username", |ui| {
                ui.add_sized([360.0, 32.0], egui::TextEdit::singleline(username));
            });
            secret_form_field(ui, language, "密码", "Password", password, 360.0);
        }
        AuthDraft::AwsSigV4 {
            access_key,
            secret_key,
            session_token,
            region,
            service,
        } => {
            secret_form_field(ui, language, "Access Key", "Access key", access_key, 360.0);
            secret_form_field(ui, language, "Secret Key", "Secret key", secret_key, 360.0);
            secret_form_field(
                ui,
                language,
                "会话令牌",
                "Session token",
                session_token,
                520.0,
            );
            form_field(ui, language, "区域", "Region", |ui| {
                ui.add_sized(
                    [240.0, 32.0],
                    egui::TextEdit::singleline(region).hint_text("us-east-1"),
                );
            });
            form_field(ui, language, "服务", "Service", |ui| {
                ui.add_sized(
                    [240.0, 32.0],
                    egui::TextEdit::singleline(service).hint_text("execute-api"),
                );
            });
        }
        AuthDraft::Unsupported(_) => {
            ui.colored_label(
                palette.warning,
                tr(
                    language,
                    "该认证类型保持原配置，但当前不可编辑",
                    "This authentication type is preserved but not editable",
                ),
            );
        }
    }
    error
}

pub(crate) fn secret_form_field(
    ui: &mut egui::Ui,
    language: Language,
    chinese: &str,
    english: &str,
    secret: &mut crate::draft::SecretDraft,
    width: f32,
) {
    form_field(ui, language, chinese, english, |ui| {
        ui.add_sized(
            [width, 32.0],
            egui::TextEdit::singleline(&mut secret.replacement)
                .password(true)
                .hint_text(if secret.configured {
                    "••••••••"
                } else {
                    ""
                }),
        );
    });
}

pub(crate) fn form_field(
    ui: &mut egui::Ui,
    language: Language,
    chinese: &str,
    english: &str,
    add: impl FnOnce(&mut egui::Ui),
) {
    ui.horizontal(|ui| {
        ui.add_sized(
            [88.0, 32.0],
            egui::Label::new(match language {
                Language::Chinese => chinese,
                Language::English => english,
            }),
        );
        add(ui);
    });
}
