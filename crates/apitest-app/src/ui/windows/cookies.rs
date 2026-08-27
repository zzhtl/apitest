use chrono::{DateTime, Utc};
use eframe::egui::{self, RichText};

use crate::app::ApiTestApp;
use crate::theme::UiExt;
use crate::ui::widgets::{Tone, badge, empty_state, icon_button};

impl ApiTestApp {
    /// The cookies every HTTP request currently shares, grouped by domain.
    pub(crate) fn cookies_window(&mut self, context: &egui::Context) {
        let mut open = self.show_cookies;
        let cookies = self.cookie_jar.list();
        let mut remove = None;
        let mut clear = false;
        egui::Window::new(self.tr("Cookie 管理", "Cookies"))
            .open(&mut open)
            .collapsible(false)
            .default_size([680.0, 420.0])
            .show(context, |ui| {
                let palette = ui.palette();
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(self.tr(
                            "响应中的 Set-Cookie 会写入这里，并在后续请求中自动携带；持久 Cookie 会跨重启保存。",
                            "Set-Cookie responses land here and are sent on later requests; persistent cookies survive restarts.",
                        ))
                        .small()
                        .color(palette.muted),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .add_enabled(
                                !cookies.is_empty(),
                                egui::Button::new(self.tr("全部清除", "Clear all")),
                            )
                            .clicked()
                        {
                            clear = true;
                        }
                    });
                });
                ui.separator();
                if cookies.is_empty() {
                    empty_state(
                        ui,
                        self.tr("暂无 Cookie", "No cookies yet"),
                        self.tr(
                            "发送请求后，服务器返回的 Cookie 会显示在这里",
                            "Cookies returned by servers will show up here",
                        ),
                    );
                    return;
                }
                egui::ScrollArea::vertical().show(ui, |ui| {
                    let mut current_domain: Option<&str> = None;
                    for cookie in &cookies {
                        if current_domain != Some(cookie.domain.as_str()) {
                            current_domain = Some(cookie.domain.as_str());
                            ui.add_space(6.0);
                            ui.label(RichText::new(&cookie.domain).strong());
                        }
                        ui.horizontal(|ui| {
                            ui.add_space(12.0);
                            ui.label(RichText::new(&cookie.name).monospace());
                            let value = if cookie.value.chars().count() > 42 {
                                let truncated =
                                    cookie.value.chars().take(42).collect::<String>();
                                format!("{truncated}…")
                            } else {
                                cookie.value.clone()
                            };
                            ui.label(RichText::new(value).monospace().color(palette.muted));
                            ui.label(RichText::new(&cookie.path).small().color(palette.muted));
                            match cookie.expires_at.and_then(DateTime::from_timestamp_secs) {
                                Some(expires) => {
                                    ui.label(
                                        RichText::new(format!(
                                            "{}",
                                            expires
                                                .with_timezone(&Utc)
                                                .format("%Y-%m-%d %H:%M UTC")
                                        ))
                                        .small()
                                        .color(palette.muted),
                                    );
                                }
                                None => badge(ui, "Session", Tone::Info),
                            }
                            if cookie.secure {
                                badge(ui, "Secure", Tone::Success);
                            }
                            if cookie.http_only {
                                badge(ui, "HttpOnly", Tone::Info);
                            }
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if icon_button(ui, "trash-2", self.tr("删除", "Delete"))
                                        .clicked()
                                    {
                                        remove = Some((
                                            cookie.domain.clone(),
                                            cookie.path.clone(),
                                            cookie.name.clone(),
                                        ));
                                    }
                                },
                            );
                        });
                    }
                });
            });
        self.show_cookies = open;
        if let Some((domain, path, name)) = remove {
            self.cookie_jar.remove(&domain, &path, &name);
            self.cookies_dirty = true;
        }
        if clear {
            self.cookie_jar.clear();
            self.cookies_dirty = true;
        }
    }
}
