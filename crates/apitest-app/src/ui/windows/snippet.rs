use apitest_core::ProtocolSpec;
use apitest_interop::{CodeLanguage, generate_code};
use eframe::egui::{self, RichText};

use crate::app::{ApiTestApp, SnippetCache};
use crate::state::action::ToastKind;
use crate::theme::UiExt;
use crate::ui::widgets::icon_button;

const LANGUAGES: &[(CodeLanguage, &str, &str)] = &[
    (CodeLanguage::Curl, "cURL", "request.curl.txt"),
    (
        CodeLanguage::JavaScriptFetch,
        "JavaScript fetch",
        "request.js",
    ),
    (
        CodeLanguage::PythonRequests,
        "Python requests",
        "request.py",
    ),
    (CodeLanguage::RustReqwest, "Rust reqwest", "request.rs"),
];

impl ApiTestApp {
    /// The snippet for the selected request, or why one cannot be produced.
    ///
    /// Only HTTP has a code generator; the other protocols carry no equivalent
    /// client snippet, so say so instead of showing an empty window.
    fn snippet_source(&self) -> Result<String, String> {
        let Some(request) = self.requests.get(self.selected) else {
            return Err(self.tr("没有可生成的请求", "No request selected").into());
        };
        match request.edited_protocol() {
            ProtocolSpec::Http(spec) => Ok(generate_code(&spec, self.snippet_language)),
            other => Err(match self.language {
                crate::i18n::Language::Chinese => {
                    format!("{:?} 协议暂不支持生成代码片段", other.kind())
                }
                crate::i18n::Language::English => {
                    format!("Code snippets are not available for {:?}", other.kind())
                }
            }),
        }
    }

    pub(crate) fn snippet_window(&mut self, context: &egui::Context) {
        let mut open = self.show_snippet;
        // Regenerate only when the request, language or edit revision moved;
        // the revision advances via the edit sweep, at most every 250 ms.
        let key = self.requests.get(self.selected).map(|request| {
            (
                request.id(),
                self.snippet_language,
                request.autosave.current_revision(),
            )
        });
        let cached = key.is_some_and(|(request, language, revision)| {
            self.snippet_cache.as_ref().is_some_and(|cache| {
                cache.request == request && cache.language == language && cache.revision == revision
            })
        });
        if !cached {
            let source = self.snippet_source();
            self.snippet_cache = key.map(|(request, language, revision)| SnippetCache {
                request,
                language,
                revision,
                source,
            });
        }
        let source = match &self.snippet_cache {
            Some(cache) if key.is_some() => cache.source.clone(),
            _ => self.snippet_source(),
        };
        let mut copy = None;
        let mut export = None;
        let mut selected = self.snippet_language;
        egui::Window::new(self.tr("生成代码片段", "Generate code snippet"))
            .open(&mut open)
            .collapsible(false)
            .default_size([720.0, 520.0])
            .show(context, |ui| {
                ui.horizontal(|ui| {
                    for (language, label, _) in LANGUAGES {
                        ui.selectable_value(&mut selected, *language, *label);
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if icon_button(ui, "copy", self.tr("复制", "Copy")).clicked() {
                            copy = source.as_ref().ok().cloned();
                        }
                        if icon_button(ui, "download", self.tr("保存到文件", "Save to file"))
                            .clicked()
                        {
                            export = source.as_ref().ok().cloned();
                        }
                    });
                });
                ui.separator();
                match &source {
                    Ok(code) => {
                        egui::ScrollArea::both().show(ui, |ui| {
                            ui.add(
                                egui::Label::new(
                                    RichText::new(code).monospace().color(ui.palette().text),
                                )
                                .selectable(true)
                                .wrap_mode(egui::TextWrapMode::Extend),
                            );
                        });
                    }
                    Err(message) => {
                        ui.colored_label(ui.palette().warning, message);
                    }
                }
            });
        self.show_snippet = open;
        self.snippet_language = selected;
        if let Some(code) = copy {
            context.copy_text(code);
            self.toast(ToastKind::Success, self.tr("已复制", "Copied"));
        }
        if let Some(code) = export {
            let file_name = LANGUAGES
                .iter()
                .find(|(language, _, _)| *language == selected)
                .map(|(_, _, file_name)| *file_name)
                .unwrap_or("request.txt");
            self.export_text(file_name, "Text", &code);
        }
    }
}
